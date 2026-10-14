# 250. Highlights by the frame's own range (2026-10-05)

§146 kept highlights in fixed stops of light and left the other way,
the frame's own range, to testers. The report came back that our
highlights still does not reach into the mid-tones as Lightroom's
does, and §146's measurements already said why: under the median
ours did nothing where Lightroom's takes a tenth or two of a stop, and
the low-contrast watch got two thirds of Lightroom's pull, both the
frame's range at work. We considered two other answers. Exposure is
not one: it moves every tone alike, where highlights is a slope
strongest in the bright mid-tones. A Midtones slider would sit on top
of exposure, contrast and the parametric curve's Lights and Darks, and
would still leave Highlights unlike Lightroom's. So this is (a).

**The range.** The guide plane already holds the frame's regions as
smoothed log luminance; `Guide::range` is its median and its 99th
percentile, read once when the plane is made (`plane_range`). A
region rather than a speck, since the plane is smoothed, and short of
the very top so that a lamp does not stand for the frame. The whole
frame before its geometry, as the plane is, so a crop does not move
it. The viewport gets the two numbers in a new uniform beside the
plane's; with no plane, both sides take a typical frame's,
`FRAME_TYPICAL` (a median 0.9 stops under grey and a top 1.9 over at
the default develop).

**The span.** `highlights_span` brings the range to the picture's
exposure and contrast, as a region's reading is brought, and takes
the median to the top as the span, held between 1.6 and 3.5 stops and
hung from the top. The picture's exposure and not the pixel's: a mask
that brightens a region moves it up the ramp, and the global exposure
moves the frame and its ramp together. Held because the frames range
from the watch's 0.8 stops to the low-key interior's 5.6: the least
keeps the slope under one at the slider's limit, the most keeps the
ramp off the interior's shadows, whose median sits five stops under
its lamps.

**The shape**, over the span (0 at its foot, 1 at the top): rising by
a smoothstep from -0.3 to 0.75 to a peak weight of 0.42, easing back
by half of that from 0.75 to 1.2, and holding there. A typical
frame's median takes a fifth of the peak, a region a stop under it
nothing, mid grey two thirds. Peak slopes at the slider's limit and
the least span are 0.75 rising and 0.875 easing.

**Shadows, then highlights.** §98's guard held because the two ramps
were fixed and overlapped only where each was gentle. Now the
highlights ramp can sit on the steepest part of the shadows ramp in a
dark frame, and side by side the two fold: the sweep finds the curve
turning back at shadows +2 and highlights -2 with the span's foot 3.5
stops under grey. So `shape` lifts by shadows first and reads
highlights at the region where shadows put it. Each is monotonic on
its own, so one after the other is too, wherever the span is. A test
sweeps the span's foot from six stops under grey to six over at four
widths, on all four corners. With either slider alone nothing changes
from adding them; together, a shadow lifted far enough meets the
highlights ramp, which is the order an editor would expect.

**Measured.** The median change in display stops at -2 by percentile
band of the untouched render, from the 55th up, against Lightroom's
-100 from §146 (four of the frames, ours from §146 reproduced
first):

| frame | before (§146) | now | Lightroom |
|---|---|---|---|
| watch | -0.44 -0.52 -0.56 -0.61 -0.58 | -0.79 -0.75 -0.65 -0.52 -0.51 | -0.74 -0.69 -0.54 -0.46 -0.46 |
| lighthouse +1 | -0.03 -0.65 -0.66 -0.60 -0.37 | -0.38 -0.56 -0.46 -0.40 -0.44 | -0.22 -0.55 -0.54 -0.48 -0.33 |
| portrait | -0.40 -0.63 -0.65 -0.65 -0.61 | -0.74 -0.73 -0.63 -0.46 -0.31 | -0.55 -0.62 -0.53 -0.46 -0.31 |
| sunset | -0.28 -0.43 -0.07 0 0 | -0.06 -0.29 -0.11 0 0 | -0.18 -0.42 -0.11 -0.06 -0.06 |

The summed distance from Lightroom over the twenty bands goes from
2.34 stops to 1.54. The watch, the frame (a) was for, is within a
tenth everywhere; the portrait's top now matches and its upper
mid-tones run a tenth or two strong; the sunset, whose bright regions
sit high in a wide span, gets a little less than before in its 55th
to 85th. Under the median, the 40th to 55th band of the lighthouse and
the portrait now takes -0.23 and -0.19 where it took nothing; §143 put
Lightroom's at -0.1 to -0.2. The low-key interior, with no Lightroom
highlights export, moves from its 85th percentile up to its 70th.

**Not.** Whites stays in fixed stops; the roadmap pairs it with this
and §150 found the same shape in it, and it is the next step if this
one reads right. The Lightroom importer's scale is unchanged, -100 is
still -2. No schema change: a stored highlights opens a little
different on most frames, as §98's retune did.

**Standing (2026-10-13).** Decided and landed: the frame's range ships as
Highlights, and Whites follows it (§315), after a trial beside a
Brightness slider and the fixed ramp on one build; the user leaned to
the adaptive ramp and found Brightness and it alike on portraits. What
follows is the standing as written on 2026-10-05.

**Standing (2026-10-05).** The trial is built on a branch off 0.4.0
(`highlights-frame-range`) and was tried on 2026-10-05; we were torn
between the two readings and the decision is open. This section is
the record of the design and its measurements; the roadmap's open
question on the highlights slider points here, and the feel track's
highlights-and-whites item carries the decision.
