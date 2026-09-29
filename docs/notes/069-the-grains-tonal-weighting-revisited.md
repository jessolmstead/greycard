# 69. The grain's tonal weighting, revisited (2026-09-18)

§33 weighted the grain towards the shadows (`1 - 0.65 luma^1.5`),
and §61 found it absent in a sky, where film grain shows most. It
was backwards: a scanned negative's grain is noise in density, and
density tracks log exposure only through the film's straight line,
so in encoded value the noise is flattest through the midtones and
falls off in the toe (near the base, little density left to vary)
and the shoulder (compressing to white).

`Grain::weight(luma)`: a bump `(l/0.6)^1.5 (1-l)/0.4` that peaks
exactly at luma 0.6, a touch above mid grey where a face or a bright
sky sits, over a floor of 55 percent of the peak at both ends, so
deep shadow and a highlight keep grain and never lose it, as a
print does not. The peak is 0.8 of `GRAIN_SCALE`, 0.096 against the
old curve's 0.092 at mid grey, so an edit's Amount reads about the
same there. The 1.5 power is `r * sqrt(r)` on both paths, as the
old code had it, since the shader's general `pow` is not held to
the precision `sqrt` is. The shader's `grain_apply` is the same
curve with the constants inlined; the test asks that the midtones
beat both ends, that neither end falls under half the peak, and
that a scan of twenty-one lumas never dips below the ends.

Checked on the §61 frame with cubic grain at 0.4 and everything
else default: a deep-shadow rock patch's luma standard deviation
fell from 3.92 to 2.66 of 255; a hazy sky patch at luma 0.44, left
of the peak where the old curve was already near its own maximum,
barely moved (3.20 to 3.06); at luma 0.8, a bright sky, the curve
gives 34 percent more than the old one did, against 56 percent less
in the deepest shadow. CPU against GPU at the same crop and window:
0.45 percent RMSE with the new curve, 0.79 with the old, 0.26 with
grain off, so the agreement tightened.
