# 22. Point curves (2026-09-06, late)

The next item off the roadmap. The sliders of §19 are a parametric
curve in all but name, so what was missing was the point curve: a
master curve on all three channels and one per channel, drawn on the
histogram, dragged with the mouse. `crates/greycard-edit/src/curve.rs`
holds the edit and its meaning; the viewport shader and `finish.rs`
apply it; a CURVES section on the panel edits it.

**Where it acts.** On the encoded working-space picture, after the
tone curve and before the matrix to the output space: encode, look
up, decode, then the matrix and the encode for the output as before.
Putting it after the output matrix would make an export in Display P3
differ from the viewport in sRGB; putting it in linear light would
make a curve drawn on a display-referred histogram mean something
else. The encoded working space is the one domain both the panel and
every output share. A consequence worth knowing: the per-channel
curves act on Rec.2020 primaries, as darktable's RGB curve does in its
working profile, so a red curve pulled hard sends mid grey out of the
sRGB gamut and the sRGB matrix clips it. The test says so.

**The curve.** A monotone cubic through the points, Fritsch and
Carlson's tangents, so a rising set of points gives a rising curve
with no overshoot between them, and flat beyond the end points. Points
are sorted by x on use; the panel keeps a hundredth of x between
them, the ends fixed at x = 0 and x = 1 and free in y. Each channel
goes through its own curve, then the master. The set bakes to a table
of 256 entries, red, green and blue with the master composed in and
the master alone in the fourth place, read linearly between entries
by both the shader (a 4 KB uniform, one per draw like the parameters)
and the CPU; the histogram's key includes it, so the bins follow a
drag. Baked every frame, which is microseconds.

**The editor.** The panel's curve is a picture drawn on the CPU, 256
pixels square: the histogram behind in the channel's tint (all three
for the master), a grid at the quarters, the diagonal, the curve two
pixels thick, the points as squares. A `CurveEditor` control maps the
pointer to curve coordinates and the window does the rest: a press on
a point takes it, a press elsewhere adds one, a drag moves it within
its neighbors, release records the edit through the ordinary
view-changed path (so the sidecar and the history see one entry per
drag, not one per pixel), a double-click removes a point, Reset puts
the channel's line back. Drawing the picture ourselves rather than in
Slint paths keeps the histogram, the grid and the curve in one place
and costs nothing measurable.

**Checked.** With a master curve, a red curve and a blue curve in a
sidecar, the viewport at 1:1 and the export's crop differ by 0.07
percent RMSE, the display profile off on both, and differ from the run
without curves by 9 percent; `GREYCARD_UI_CURVE=FILE` dumps the panel's
picture, looked at. The edit round-trips through the sidecar as plain
point lists.

**Not yet.** Color curves against luminance (r/g and b/y, the Lab
`a` and `b` curves of our list) are a different domain and a
different picture; on the roadmap. A curve on luminance alone, hue
held, would be the master curve applied as a gain rather than per
channel; not built until someone misses it.
