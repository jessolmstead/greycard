# 85. The Light sliders: a tone equalizer and a white point (2026-09-19)

The roadmap's complaint, in our own words: whites did basically
nothing, highlights did not recover a frame with a lot of range, and
shadows was about right. §19's sliders were gains by the pixel's own
luminance over ramps at 0..3, -3..0 and 2..5 stops over mid grey.
Two things were wrong with that, and they are fixed differently.

**Highlights and shadows read a region, not a pixel.** A gain by the
pixel's own luminance flattens whatever it touches: pulling
highlights down pulls the bright side of every edge and not the dark
side, so texture inside a highlight loses its contrast instead of
being recovered. The two shifts now read a guide plane: the log
luminance of the developed picture over mid grey, in stops, averaged
into cells at about a thousand texels along the long edge (a
geometric mean, so a specular speck moves a cell by its share and not
its height), then the guided filter (He, Sun and Tang; ported into
`greycard_core::guided` from the learned masks' refinement, since
both want it and neither crate may depend on the other) against
itself with a radius of a fortieth of the long edge and a
regularizer of two stops squared. That is darktable's tone equalizer
in outline. Within a region the shift is one constant, so a gain is
all it is and the texture keeps every bit of its contrast; across a
region boundary each side is pulled on its own. Made once with the
base after the lens and the defringe, before the retouch and the
sharpen, cached with it, an R16Float texture for the viewport and the
same plane for the export, read through the same bilinear on both
paths and through the geometry, so a crop or a turn finds the same
region. The ramps move to where the scene is: highlights 0..2.5
stops, full at scene white rather than nine tenths, shadows -3..0 as
before. The regularizer was chosen on the lighthouse frame: at 0.6
the plane is the picture at an eighth scale and the shift follows the
water's sparkle; at 5 and above a dark object smaller than the window
is read as part of the sky behind it and the halo doubles; at 2 a
tree line and a rock mass keep their edges and the texture inside
them does not.

**Whites is a white point, not a third gain.** The branch first made
it a region gain over 1..2.75 stops, which overlapped highlights
almost entirely: two gains on the same stretch can always fold, so
§19's guard that no corner of the sliders turns the curve back had to
be dropped and an inversion pinned. And it gave whites no job of its
own. What a photographer means by whites is where the top of the
scale lands. Two designs were weighed. Replacing the shoulder with a
curve that reaches display white at a chosen point was the first,
and it was wrong for a reason worth recording: Narkowicz's ACES fit
reaches display white only 5.3 stops over mid grey, so scene white
(2.47 stops, §19) renders at 0.80 in linear light, 231 of 255, and a
shoulder that hit white at scene white would brighten everything
over mid grey in every picture at zeroed sliders. A change to the
default look, not a slider fix. The second, which is what landed,
keeps the shoulder and puts the white point in front of it, pivoted
at mid grey: a luminance `whites` stops under scene white is brought
to scene white and everything over mid grey with it, by a power on
the luminance whose exponent is `2.47 / (2.47 - whites)`; nothing at
or under mid grey moves. Whites at minus one puts the clip a stop
further out and compresses the top two and a half stops evenly
towards grey, which is highlight compression with no halo because it
is global; plus one clips a stop earlier and stretches the top. A
linear stretch pivoted at mid grey was the other form; it leaves a
knee at grey whose slope ratio is 0.45 at minus one, where the
power's is 0.71 and is eased away, the exponent running from one at
mid grey to its value a stop over by a smoothstep, so the curve is
smooth through grey (a test holds the slope ratio either side to a
tenth of a percent) and exact from a stop over it, which is
everywhere the white point can be at the slider's range. Monotonic
while the exponent is over 0.41, whites over minus three and a half.
A gain on the luminance rather than a power per channel, as the
shifts are, so hue holds. With whites out of the gains the two
shifts do not overlap and each has a peak slope of 0.9 at its limit,
so the guard is back in full: all sixteen corners sweep monotonic.

**Checked.** The viewport at 1:1 and the export's crop of
`5M0A3976.CR3` under all four sliders off center (highlights -1,
shadows +0.5, whites -1, blacks -0.1, contrast 1.2, +0.3 EV) differ
by 0.055 percent RMSE, 0.14 of 255, and 0.02 of 255 on average, the
display profile off on both. The lighthouse frame at +1.5 EV under
whites -1, 0 and +1: the picture's mean 172.7, 174.9 and 178.9 of
255, the sky and the tower's paint coming down and going up while
the rocks and the water, at and under grey, do not move. At 0 EV the
frame is nearly all under mid grey, and whites changes only the
water's sparkle, which is the design and is what Lightroom's does on
such a frame. Schema version 3 for the change of meaning, an
identity migration, argued in `migrate`'s doc.

**Open, and it decides the feel as much as any slider.** The
shoulder's own white at 5.3 stops. Scene white renders at 231 of
255 by default, and every camera's JPEG renders it at 255, so the
top of every picture is dimmer here than the camera's by design.
Whites +1 brings the clip to 250 and -1 to 214, which is asymmetric
because of that asymptote. Whether the default shoulder should reach
white at scene white is a change to the default look of the whole
editor and wants the reference frames of the next item before it is
made, not a slider's justification.

**How the feel gets settled.** Eight reference frames with a
Lightroom edit each (backlit, a portrait, snow, night, foliage, a
sunset, the lighthouse, flat overcast) and the greycard values that
match; where greycard needs twice the deflection or a different
slider, the mapping is wrong and the frame says which. A check that
the default develop's mean brightness matches the embedded JPEG's
across the test set. A slider response that puts the everyday work
in the first third of the travel. An Auto from the histogram's
percentiles to exposure, whites and blacks. Each of these lands on
both paths or on neither.

**Measured, in more detail.** The branch's own account of the same
change, kept for its numbers. Where it says "the tone equalizer" it
means highlights and shadows; whites is the white point above.

**What the sliders do now, measured.** All on 4Z4A3525 at +1.9 EV,
whole 45 MP frames, RMSE against the untouched export; the old
behavior from the same binary with the §19 shift switched back in for
the comparison.

- **Whites at -1:** 0.058 percent under §19 (0.15 of 255), **3.61
  percent now**. On a 600x400 block of open sky the old whites changed
  the mean log luminance by *nothing at all, to the bit* — that is the
  roadmap's "whites do basically nothing", exactly — and the new one
  by 0.263 display stops. Whites at +1 is 5.72 percent and +0.380
  stops on that block; the asymmetry is the shoulder's asymptote.
  A 500x400 block of rock well under mid grey is *identical to the
  bit* at -1, 0 and +1, which is the pivot doing what it says.
- **Highlights at -1:** 4.41 percent under §19, **5.19 percent now**,
  and on that sky block 0.321 display stops then against 0.432.

**And local contrast, which is the point.** Highlights -1 with shadows
+1, the standard deviation of the log luminance over a block, in
stops:

- 4Z4A3525, water sparkle (400x300 at 5000,4000): 0.746 untouched,
  0.553 under §19 (down 26 percent), 0.648 now (down 13).
- 4Z4A3525, the rock mass (500x400 at 5200,2600): 1.389, 0.971 (down
  30), 1.188 (down 14) — and the block's mean went from -5.480 to
  -4.233 under §19 and to -4.145 now, so the new one lifts the region
  *further* while losing half as much of its texture.
- 4Z4A2764, the branch tangle (600x500 at 2500,1200): 1.279, 0.807
  (down 37), 1.110 (down 13).
- 4Z4A2764, the cliff in shade (500x500 at 300,2000): 1.329, 0.966
  (down 27), 1.147 (down 14).

The residual loss is the ACES shoulder's, not the shift's: lifting a
region moves it into a flatter part of the curve.

**Halos, at 1:1 on a backlit edge.** The lighthouse frame with
highlights at -1 and shadows at +1, a horizontal scan at y = 260
across the dark red dome (x 2870 to 3060), which sits alone in a flat
sky — the worst case the frame offers. Under §19 the sky's shift is a
flat -0.32 display stops everywhere, dome or no dome. Under the tone
equalizer the sky immediately beside the dome takes -0.237 on the left
and -0.281 on the right against -0.424 well away from it: a band about
0.19 stops brighter than the far sky, decaying over roughly 500
pixels, six percent of the frame's width. At the conifer canopy (y =
1000, x from 2190) the same measurement gives 0.18 stops over about
200 pixels.

My reading: it is there in the numbers and it is not a halo to the
eye. In levels, the sky 180 pixels from the tower is 166,195,222
against 156,190,218 well away from it, and in the untouched frame the
same two places are 188,212,233 and 186,212,232 — so the band is ten
levels of 255 in the red channel, five in green and four in blue,
spread over 180 pixels of a sky otherwise flat to two. Ten levels over
that distance has no edge for the eye to catch; what it makes is a
wide soft brightening centered on the tower, not a rim around it, which
is what a guided filter's transition looks like against a box blur's.
Pulled up hard with an auto-level it is findable; at 1:1 on the
picture itself, side by side with the untouched frame, I cannot see
it, and neither the dome's edge nor the conifers show a line.

The shape is worth saying plainly, because it is the guided filter
earning its place: the transition is a *gradient over the region*, not
an overshoot at the boundary. A box blur of the same radius would put
a bright rim on the sky side and a dark one on the tree side, which is
the halo people mean.

What *is* visible is the other side of the same coin, and it is the
honest cost of the change: a dark thing smaller than the guide's
window is treated as part of what surrounds it, so it gets much less
of the shadows lift. On the dome itself (200x100 at 2850,110) §19
raised the mean log luminance from -4.450 to -3.454, a full stop, and
the tone equalizer raises it to -4.123, a third of one; at the thin
branches against the sky at x = 2190, §19 lifts 0.161 stops and the
tone equalizer takes 0.028 down. Whether that is a loss depends on
what the lift was doing: on the dome §19 also took 36 percent of its
internal contrast with it (2.059 stops of spread down to 1.326) where
the tone equalizer takes 16 (to 1.726), which is why the dome keeps
its depth in the new frame and looks washed in the old. The dark
conifer mass, big enough to be its own region, is lifted *further*
than §19 lifted it (-6.225 untouched, -5.033 under §19, -4.929 now)
and keeps more of its texture too (2.338, 1.811, 1.970). So: large
dark regions do better than they did, and a small dark object inside a
bright one no longer gets its own stop. A mask is the way to find one
of those.

**Cost.** 45 MP (8192x5464), sixteen-core desktop, best of three. The
guide plane is 17 to 21 ms, making a 1024x683 plane at eight source
pixels to a texel, against a develop of 1.31 to 1.38 s: one and a
third percent, and it is O(n) — the box means are running sums, so the
radius is free. The finish of the whole frame to eight bits is 0.61 s
with highlights and shadows at zero, where the plane is not read at
all, and 0.65 with them set: forty milliseconds for a bilinear lookup
and the position mapping per pixel, six percent of the finish. Whites
costs nothing extra — it is global and never touches the plane, which
is why `reads_guide` does not count it. The interactive path is
unmoved, because a Light slider does not develop: the viewport redraws
in 0.1 to 0.4 ms as it did, and the frame that uploads a develop
carries 1.4 MB of guide beside the picture's 358.

**How the radius and the regularizer were chosen.** Swept on the
lighthouse frame at radius 12, 25 and 50 texels and regularizer 0.6,
2, 5 and 15 stops squared, judging the plane itself, the texture kept
in the blocks above and the halo at the dome.

At 0.6 the plane is simply the picture at an eighth scale — every
trunk, every figure, the water's sparkle — so the shift follows
texture and the water block keeps only 0.602 of its 0.746 against
0.648 at 2. At 5 the block does better still, 0.671, but the dome's
halo grows from 0.19 stops to 0.25 and the dome starts to be read as
sky. At 15 the plane is a blur and the halo is the picture's. Radius
12 at regularizer 2 gives a *taller* halo in a narrower band, 0.24
stops over 250 pixels, which is more visible than the wide gentle one,
not less; radius 50 at 5 spreads the halo to 0.11 stops but reads the
dome itself 0.20 stops down, losing it as its own region entirely. 25
and 2 is where a tree line and a rock mass keep their edges and what
is inside them does not.

**What was tried and did not pay.** Whites as a third region gain over
1 to 2.75 stops: it worked in the sense that the slider finally moved
the picture, but it duplicated highlights, cost the monotonic guard,
and the honest reading of the numbers afterwards was that no
combination of ramps could give two overlapping gains and keep the
curve. Computing the plane at full resolution: the guided filter is
O(n) either way, but a 45 MP plane is 180 MB of floats plus as much
again for the prefix sums, and 90 MB of texture, for a signal whose
whole point is that it carries nothing at the picture's scale — the
eighth-scale plane is indistinguishable in the output and costs 1.4
MB. Making the plane after the sharpen so it describes exactly the
picture being finished: the sharpen does not move a region's mean
luminance by anything a texel of eight pixels can see, and it would
put the plane on the sharpen slider's path. Scaling old sidecars'
numbers to reproduce the old rendering: see the schema above.

**Not.** No per-band tone equalizer with a curve over the exposure
bands, which is what darktable's panel actually offers; four sliders
was our instruction and the panel is unchanged. No mask on the
guide plane and no way to see it in the interface — a "show the guide"
overlay would be cheap and might be worth it later. No second
iteration of the guided filter, which sharpens its edge preservation
at twice the cost; the single pass measured well enough. And the
shifts still read the picture before the mixer and the black and white
conversion, so a band pulled down hard in the mixer does not move
which region the tone equalizer thinks a pixel is in. That is
defensible — the guide is the scene's, not the look's — but it is a
choice, not a law.
