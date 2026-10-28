# §448. The fill's middle against the surface round it

*2026-10-28.* With §447 the mast's three fills read at level across
the edge (gains of 1.002 and 0.992, within their error of none) and
still looked wrong in the mast's footprint. Measured on the dumped
inputs, the error is the fill's own interior: Patch is not light at
all but 2 to 2.5 percent dark 30 to 60 px in with blue 3.5 percent
short, a sky gone grey and warm that reads as a washed patch; Fast and
Best keep a bulge of 1 to 2 percent in their middle with red up to 5
percent over. The edge, read two pixels in where the models copy the
sky faithfully, cannot see any of it. The hole was not thin (the brush
was an oval of 186 by 250 px) and not tight (the picture sits on a
smooth fit of the sky within 0.1 percent from 3 to 40 px out).

## The surface

`remove::middle::evened` runs after `matched` on both paths and before
the mottle step. A band 4 to 32 px out from the hole is low-passed,
and its locally smooth pixels are fitted per channel with a quadratic
in logs, robustly (four biweight passes with the fit's sigma held to
the band's own grain, read from residuals six pixels apart), so a
second surface the hole crosses, the hill under the sky, falls out as
an outlier. What the band shows past the quadratic, a soft shadow or
a cloud's edge, is carried a short way into the hole, two box passes
of radius 8, so the reference is what the picture shows, not the
quadratic alone. Trust comes from the fit's residual (full at 2
percent in the worst channel, none at 3), the band's smooth share
(none at 30 percent, full at 50), at least 200 inliers on four of
eight octants, and the band's soft share, the pixels off the fit but
within 8 percent of it: past 4 to 7 percent the band holds a soft
surface a wide hole's fill would continue where the carry cannot
reach, and the correction fades out deep inside.

## The correction

Inside the hole the fill's low-pass is compared with the surface where
the fill is smooth and within 8 percent of it on every channel, so the
hill the model drew and an object it kept are not read. The difference
is smoothed by two box passes of radius 10, shrunk by three sigma of
its own noise, held to 5 percent per channel, and divided out of the
whole hole round those pixels, with no fade at the edge, since a
stroke is all edge. A textured surround (the walls, foliage) does not
fit the quadratic and the fill is left as it is.

## Measured

Fill against the sky, luma, after the edge match then after this:

| fill | 1 to 3 px | 10 to 20 | 30 to 40 | 60 to 100 |
|---|---|---|---|---|
| Patch | -0.69 to +0.08 | -1.51 to -0.22 | -2.44 to -0.33 | -1.88 to +0.14 |
| Fast | +0.08 to -0.40 | +0.87 to +0.26 | +0.73 to +0.15 | +1.74 to +0.47 |
| Best | -0.35 to -0.42 | +0.27 to +0.06 | +0.51 to +0.17 | +0.78 to +0.42 |

Patch's blue 30 to 40 px in goes from 3.5 percent short to 0.3; Fast's
red deep in from 5 percent over to 1.5. The walls and the sky frame
are untouched, bit for bit (fit 2.6 to 3.5 percent, smooth share 8 to
30). A 7 px stroke 2 to 3.5 percent light, which the edge never
reads, comes within 0.3 percent; a stroke under a 6 percent cloud
wisp is corrected under the wisp and off it alike. A fill at level
with grain moves under 0.005 at any pixel over 300 draws. A 1024²
window with a radius-400 hole takes 65 ms in release.

## Known limits

A shadow wider than the carry's reach and shallow enough for the fit
to absorb (50 px wide, 3 percent deep) is still lifted about 1
percent. Deep inside a wide hole on a smooth sky the quadratic does
not fit, a sun's glow, the fill can move up to 2 percent at a spot;
fitting with one octant left out and shrinking by that octant's
prediction error would bound it.
