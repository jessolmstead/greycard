# 29. Mask handles (2026-09-07)

The gap left by §27: a placed gradient could only be deleted and
redrawn. Now the chosen shape shows itself on the view and can be
moved and reshaped. A linear gradient is drawn as three lines, at
`from`, the middle and `to`, at right angles to its direction, with
a handle on each: the middle moves the whole, the ends move
themselves. A radial one is drawn as its ellipse and the inner
ellipse where its feather begins, with a handle at the center that
moves it and one at each end of both axes: an axis end dragged sets
that radius from its distance and turns the ellipse to face it, so
reshaping and rotating are one motion rather than a separate rotate
handle. All of it is `shape_handles` and `shape_dragged` in the
window, pure functions of the shape in the masks' units, tested;
`Geometry::to_plane`, the inverse of `to_source`, maps a source point
back through a turn to the view, tested against its inverse. The
handles are the crop's `CropHandle`, which reports the pointer's
travel since the press, and the drag applies it to the shape as it
was when pressed, as the crop does (§24). Release records one history
entry. The overlay hides in crop mode and while a shape is being
placed.
