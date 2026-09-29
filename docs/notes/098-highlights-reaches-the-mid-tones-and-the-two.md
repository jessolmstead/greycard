# 98. Highlights reaches the mid-tones, and the two shifts get ±2 (2026-09-19)

Our report, after §85 had been in for a day: the highlights
slider still does not do enough, not far off, but it does not reach
quite far enough towards the mid-tones; and both shifts want more
than a stop and a half of range.

**Why it stopped short.** §85's highlights ramp was a smoothstep from
mid grey to scene white, 0..2.5 stops. A smoothstep starts slowly, so
a region half a stop over grey took a tenth of the slider and a region
a stop over it a third. Those are the bright mid-tones, a lit face or
a pale sky low on the horizon, and they are what a photographer means
by "the highlights" as often as the clip is. Nothing under mid grey
moved at all. The fix is to start the ramp a stop under mid grey:
highlights runs -1..2.5, full at scene white as before, and now gives
the region a stop over grey six tenths of the slider, half a stop over
it four tenths, and mid grey itself a fifth. That last is the change
of feel worth knowing: at the slider's limit a mid grey region moves
four tenths of a stop, where §85 held it still. The alternative that
keeps grey, a ramp of -0.5..3, gives the stop-over region four tenths
and scene white 0.94, and moves the whole curve down half a stop for a
smaller gain in reach; the complaint was reach, so the wider ramp
won. If mid grey moving under highlights turns out to be wrong in
use, that is the ramp to go back to.

**The range and the guard.** The sliders are ±2 stops instead of
±1.5. The monotonic guard of §19 and §85 is what sets the limit: a
smoothstep of `a` stops over `w` stops has a peak slope of `1.5 a /
w`, and the curve turns back where the slopes of the two shifts sum
past one. At ±2 over §85's ramps highlights alone is 1.2 and the
curve folds; over -1..2.5 it is 0.86. Shadows at ±2 over -3..0 is
exactly 1.0, which is a flat spot, so its ramp widens by half a stop
to -3.5..0, 0.86 as well; the region two stops under grey takes six
tenths of the slider where it took three quarters, and the slider's
limit still lifts it 1.22 stops against 1.11 before, so the shadows
that "were about right" got a little more at full travel and a
longer road to it. The two ramps overlap in the stop under mid grey,
and the sum of the slopes there peaks at 0.86 with both at their
limit, the same as either alone, because each is near its foot where
the other is steepest. The corner test sweeps sixteen corners at ±2
and holds. A stop-narrower ramp on either side, or ±2 over the old
ramps, fails it; the numbers were swept in a script before the
constants were set.

**Measured, on the lighthouse frame** (4Z4A3525 at +1.9 EV, its own
edit with shadows zeroed, exported at 1400 px, the display profile
off). The shift in display stops between the untouched export and the
one under highlights, by band of the untouched pixel's own display
luminance in stops over grey:

| band, stops | pixels | highlights -1 | highlights -2 |
|---|---|---|---|
| -1 .. -0.5 | 4730 | -0.01 | -0.02 |
| -0.5 .. 0 | 5648 | -0.03 | -0.06 |
| 0 .. 0.5 | 4588 | -0.05 | -0.11 |
| 0.5 .. 1 | 3355 | -0.06 | -0.13 |
| 1 .. 1.5 | 4899 | -0.15 | -0.35 |
| 1.5 .. 3 | 19921 | -0.26 | -0.69 |

Nothing under two stops down moves, to the hundredth. The bands are
by pixel and the shift is by region, and this frame's mid-tone pixels
are the rock and the water, whose regions read under grey, so the
table understates what a mid-tone *region* takes: the ramp's fifth
at grey is per region and the frame has few regions at grey. Display
stops are after the shoulder, whose slope is under one at the top,
so the sky's 0.69 display stops at -2 is more than that in the scene.
Side by side at half size the sky comes down through the three, the
tower's paint holds white, the shirts and the water's sparkle come
down a little at -2, and there is no line at the tower or the
conifers. The viewport and the export read the same constants from
`finish.rs` and `viewport.wgsl`, and the shader test suite parses
both.

**Whites too, at ±2.** We asked, and the answer is yes with
one thing to know. Whites is a white point, and its exponent
`2.47 / (2.47 - whites)` has a pole at scene white, so §85 held the
blended value half a stop short of it, at 1.97, and the slider at
±1. The slider now runs ±2 and the hold moves to 2.0, 0.47 short of
the pole, where the exponent is 5.3: at the top of the travel a
luminance half a stop over mid grey is brought to scene white, which
is a cliff and is meant to be, being the end of the slider. Minus
two is an exponent of 0.55, well inside the 0.41 the monotonic
arithmetic allows, and the corner test now sweeps whites at ±2. The
pole test's blended three still lands on the hold.

**What else moved.** The Lightroom import's clamp on the three, ±2.
The schema stays at 3: §85's bump was for a change of what the
numbers *are*, a region's shift and a white point; this is where the
same shift reaches, and nothing has shipped between. A stored -1
opens a little stronger through the mid-tones than it was saved,
which is what the report asked for.

**Not.** No change to whites, blacks or the guide plane. No
reference frames yet; the roadmap's feel item still holds those, and
this is the second retune made on one frame and a report, which is
exactly what that item is for.
