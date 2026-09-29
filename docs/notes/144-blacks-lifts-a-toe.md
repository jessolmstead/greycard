# 144. Blacks lifts a toe (2026-09-22)

§143 found the lift fogging the frame: the black point was an offset in
scene light, `-blacks` of mid grey, the same arithmetic both ways, and
at +0.3 it laid 1.7-stops-under-grey of light over every pixel. The
crush half of it matched Lightroom's -100 band for band and is kept as
it was. The lift is now `black_point`'s other branch: a gain on the
luminance, `BLACKS_LIFT` stops at the slider's top, full at and under
5 stops below mid grey and faded out by a smoothstep to nothing 1.5
stops over it. A gain and not an offset, so black stays black and a
color keeps its channel ratios; a luminance and not a region, like the
white point, since what it shapes is the curve's toe.

**The numbers came from the interior frame.** Its bands of greycard's
own picture, taken back through the shoulder to scene stops, sit at
-7.2, -7.0, -5.1, -3.9, -3.0, -1.7, +0.2 and +1.7 against mid grey;
Lightroom's +100 lifted them +2.3, +1.6, +1.3, +1.6, +1.5, +1.0, +0.3
and nothing, in display stops. Divided by the toe's own log slope
there, 1.2 to 1.6, that is about 1.1 stops of scene lift from -5 down,
0.9 at -3, 0.6 at -1.7 and a tenth or two over grey. A lift of 1.2 over
a ramp from -5 to +1.5 is that shape, and its peak slope is 0.28, so
the curve cannot fold at any setting; the sixteen-corner sweep still
holds. Blended with masks the value can pass 0.3, and at three masks'
worth the slope is 0.84, still under one.

**Measured after.** The interior at +0.3 against +100, in the bands
from the 25th percentile up: +1.64, +1.67, +1.47, +0.89, +0.11 against
+1.33, +1.55, +1.46, +0.95, +0.27. The darkest quarter takes +1.4 and
+1.2 where Lightroom's takes +2.3 and +1.6; that is a few levels of an
8-bit JPEG near black and Lightroom lifting its floor a little, and
it is left. The frame's shadows open and it stays a photograph where
it went under a veil before. The viewport draws the same lift: the fit
view against the export scaled to it differs by 1.66 of 255 on
average, -0.05 signed, where the same comparison with the slider at
zero gives 1.14.

**What else moved.** The Lightroom importer took `Blacks2012` at 0.2 per
hundred and now takes 0.3, which is what the crush and the lift both
measure. Warm Negative's `blacks: 0.1` was a matte black, a faded
negative's grey floor, and it leaned on the veil; the veil is a
legitimate look, but it is the point curve's to make, as every
Lightroom preset makes it, so the preset has `blacks` at zero and its
RGB curve starts at 0.07. The shipped-presets test, which asks that it
render softer and less saturated than the picture, holds. A stored
edit with a positive `blacks` renders its shadows lifted instead of
veiled, which is the fix, and no schema version moves for it, the
same call §141 made.
