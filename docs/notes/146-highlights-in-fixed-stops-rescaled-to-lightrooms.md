# 146. Highlights in fixed stops, rescaled to Lightroom's (2026-09-22)

§143 found greycard's highlights at -2 pulling the top of five frames
out of six about twice as hard as Lightroom's -100, doing nothing under
the median where Lightroom's takes a tenth or two, and pulling hardest
at the very top where Lightroom's eases off: its strongest pull is in
the bright mid-tones, and the brightest tones, whose place is display
white, take a half to two thirds of that. Lightroom's also adapts to
the frame, reading its own range, which is how it does the same job on
the low-contrast watch as on the sunset.

**Two ways, and the one taken for now.** (a) Place highlights and whites
by the frame's own range, which the guide plane already measures: the
familiar feel, at the cost of a slider whose stops mean something
different on every frame. (b) Keep them in fixed stops of light,
rescaled and reshaped to Lightroom's on the set: consistent, and true
to a scene-referred editor, but short on a frame like the watch whose
whole range sits within a stop or two of grey. (b) now; the choice is
put to testers before v0.2.0 and is on the roadmap.

**The shape.** `highlights_weight` is the share of the slider a region
takes by its stops over grey: §85's smoothstep from a stop under grey
to 2.5 over, now up to 0.55 rather than one, times an ease of 0.4 of
that from two stops over to display white (§145), so the weight is
0.11 at grey, 0.33 a stop over, 0.52 at its peak two stops over and
0.33 at white and past it. At the slider's -2 that is -1.04 scene
stops at the peak and -0.66 at white. Its peak slope is 0.47 rising
and 0.52 easing, so the corners' sweep still holds. The shader has the
same function; the fit view against the export at -2 on the portrait
differs by 0.86 of 255. The Lightroom importer took Highlights2012 as
a hundredth per point and now takes two, so -100 comes across as -2.

**Measured after,** -2 against -100 from the 55th percentile up:

- the wedding -0.43, -0.57, -0.60, -0.59, -0.54 against -0.63, -0.69,
  -0.77, -0.61, -0.41;
- the portrait -0.40, -0.62, -0.65, -0.65, -0.61 against -0.55, -0.62,
  -0.53, -0.46, -0.31;
- the lighthouse -0.03, -0.65, -0.66, -0.60, -0.34 against -0.22,
  -0.55, -0.54, -0.48, -0.33;
- the sunset -0.29, -0.42, -0.03, 0, 0 against -0.18, -0.42, -0.11,
  -0.06, -0.06;
- the watch -0.45, -0.52, -0.56, -0.61, -0.61 against -0.74, -0.69,
  -0.54, -0.46, -0.46.

Where it was twice Lightroom's it is now within about a tenth or two of
a stop, a little strong at the very top of the wedding and the
portrait. What (b) does not do is visible in the rest: under the
median it still does nothing where Lightroom's takes -0.1 to -0.2 (a
region under a stop below grey is outside the ramp), the watch's upper
mid-tones get two thirds of Lightroom's pull, the sunset's clipped sky
comes down to 11.6 percent at white where Lightroom's leaves 2.5, and
the GFX ferry takes half of Lightroom's, which is its baseline half a
stop too bright putting its lights further up the ramp's ease. Each of
those is the frame's own range at work, which is (a)'s argument.
