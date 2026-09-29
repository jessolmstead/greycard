# 104. Dehaze (2026-09-19)

The roadmap's Dehaze slider: one control, ±100, in the DETAIL
section, separate from the sharpen. Built in the engine as
`develop::dehaze`, a port in outline of darktable's haze removal
(`src/iop/hazeremoval.c`, Heiko Bauke, 2017), with the file header
saying so and where it departs.

**The model.** Haze is the atmosphere's own light laid over the
scene: `I = J t + A (1 - t)`, with `J` the scene's radiance, `A` the
airlight and `t` the transmission, which falls with distance. He,
Sun and Tang's dark channel prior (CVPR 2009) is the observation
that in a clear outdoor scene nearly every patch has some channel
near zero, so the darkest channel of a patch, divided by the
airlight, is a read of `1 - t`. The airlight is read off the haziest
pixels; the transmission map is smoothed so it follows the picture's
edges rather than the patches; and the scene comes back as
`J = (I - A) / t + A`, on all three channels alike, which is what
keeps hue: the recovery is a scaling about `A`, the same on every
channel.

**The algorithm as it runs.** On the linear working image, after the
retouch and before the sharpen:

1. Reduce. The picture is brought to a long edge of about 1536, each
   reduced pixel the mean of its block (four by four at 24 MP). The
   airlight, the dark channel and the guided filter's fit all
   happen here, so their cost does not grow with the picture.
2. Airlight, darktable's estimate: the dark channel (the least
   channel, then the least over a 3x3 window) is taken over the
   reduced picture; the pixels at or above its 95th percentile are
   the hazy ones; among those, the ones at or above the 99th
   percentile of luminance are the brightest; `A` is their mean
   color. Then each channel of `A` is held to at least a quarter of
   its brightest, see below.
3. Strength. The slider's ±100 maps to a strength `s` of ±0.8, not
   ±1 (`FULL_STRENGTH`). The reason is in step 5.
4. Transmission, on the reduced grid: `t0 = 1 - s * min_c(I_c / A_c)`
   with the ratio held to 0..1. No window beyond the block:
   darktable's `w1` of 6 pixels at full size is about the block, and
   a wider window spreads a dark object's "no haze" into the sky
   around it as a rim that nothing after it can take back.
5. Guided filter (He, Sun and Tang, ECCV 2010), the fast form (He
   and Sun, 2015): on the reduced grid, the transmission in each
   9x9 window (36 px at full size on a 24 MP frame) is
   fitted as a line in the luminance, `a g + b`, with the slope
   regularized toward zero by `eps = 0.0003` against the guide's
   variance; `a` and `b` are averaged over the windows a pixel is
   in. The map is never built at reduced size: `a` and `b` are read
   bilinearly at every full-size pixel and the pixel's own luminance
   closes the line, so the transmission has the picture's edges at
   full resolution for the cost of one multiply-add.
6. The bound. Every pixel's transmission is held to at least
   `1 - s * min_c(I_c / A_c)` of its own, then to `[0.2, 1]`. The
   bound is the model's consistency condition: `I >= A (1 - t)` on
   every channel, which is exactly `J >= 0`. Without it a leaf
   thinner than a reduced pixel takes the sky's transmission from
   the grid around it and comes out below black. With it, wherever
   it is the active term the recovered dark channel is
   `A d (1 - s) / (1 - s d)`: a share `(1 - s) / (1 - s d)` of what
   the pixel had, which at `s = 1` is exactly zero. That is why the
   strength stops at 0.8. At one, a review of the first build found
   the whole sky of the test frame with its darkest two channels at
   zero, a 20 to 30 px bright rim along the ridge (the guided
   window's halo, drawn at a gain of five) and green and magenta
   outlines on grass stems where the bound switched from pixel to
   pixel. At 0.8 the darkest channel keeps at least a fifth of
   itself, and on the frame those three are gone. He, Sun and Tang
   keep `omega = 0.95` of the veil for the same reason, aerial
   perspective; ours keeps more, since our bound is a hard one. The
   floor at 0.2 caps the gain at five: below that the model is
   amplifying noise and the airlight's error, not restoring
   radiance.
7. Recover: `J_c = (I_c - A_c) / t + A_c` in place, rayon over rows.

A negative slider gives `s < 0`: `t = 1 + |s| d > 1`, and the
recovery becomes a blend toward the airlight in proportion to the
dark channel, which is haze put in where there is haze already. A
clear scene has next to no dark channel to deepen, so a negative
amount on a clear frame does nearly nothing; that is what darktable's
negative strength does too, and the test says so.

**What the first frame taught.** 5M0A8082.CR3, a dusk vista of layered
ridges under a hazy sky, airlight read as neutral (R 0.329 G 0.319
B 0.322). The first cut used darktable's windows (`w1` 6, `w2` 9) in
reduced pixels on a grid of 768: at +50 the sky went deeper and the
far ridges came apart, but every branch against the sky wore a soft
halo 100 px wide, the min filter's reach at that scale, and at +100
a bright rim ran along the ridge. Widening the guided filter to
swamp the halo took the halo away and gave the leaves a purple edge
instead: pixels thinner than the grid took the sky's transmission
and `(I - A)/t` went negative. The per-pixel bound fixed the purple;
the strength cap fixed what the bound then did at the slider's end;
and with both in place the guide could be let to follow edges (a
9x9 window, `eps` 0.0003) without fringes or the lighter zone the
17x17 window left around leaf clusters. The airlight's neutrality
floor came from the clear-scene test: with nothing hazy in the
frame, the estimate lands on the brightest object, and its
near-zero channel made every pixel sharing that dark channel read
as deep haze (A of [0.48, 0.72, 0.015] on the synthetic scene).
Haze is scattered light, near neutral or warm; a channel under a
quarter of the brightest is an object.

At +100 on that frame, measured in the linear TIFF at x = 3500 down
the sky (per mille, R G B): the top of the sky goes from
(65, 76, 155) to (17, 33, 126), the middle from (104, 124, 218) to
(28, 59, 182), the horizon from (251, 251, 273) to (130, 146, 198).
The darkest channel keeps 26% of itself at the top and 52% at the
horizon; nothing is zero. In the 8-bit preview, whose tone curve
crushes the low end, the top of the sky's red reads 2 to 3, which
is where the review's "R = 0" came from as much as from the bound;
the G = 0 was the bound. What the picture shows at +100 is a deep
saturated blue sky, the sensor's noise in it at a gain approaching
five near the horizon, and a warm-to-blue hue divergence along the
sunset horizon where one airlight cannot fit a sky that runs from
orange to blue. That is the end of the slider; +50 is clean at 1:1.
Crops, before / +50 / +100: `scratchpad/dehaze/fix/crop-ridge.jpg`,
`crop-branches.jpg`, `crop-farridge.jpg`, `crop-skyline.jpg`.

**What it does to a picture with no haze.** This must be said plainly.
The dark channel prior has no way to tell a neutral surface from a
veil: a face, a grey wall, a white shirt has no channel near zero,
so the prior reads it as hazy, and the airlight is read off the
brightest of those. Measured at +50:

- P1000247.RW2, a studio portrait: the airlight is the skin
  (0.556, 0.458, 0.453), the mean transmission 0.94, and the face
  goes from a mean of (101, 73, 73) to (90, 61, 59) in the 8-bit
  preview: darker, ruddier and blotchier, with the texture of the
  skin's unevenness brought up (`scratchpad/dehaze/fix/crop-face.jpg`).
- 5M0A0504.CR3, an interior: the airlight is the sky through the
  window (1.014, 0.969, 0.879), the mean transmission 0.92, and a
  tenth of a "veil" is taken off a room that had none.
- The vista, for comparison: airlight (0.329, 0.319, 0.322), mean
  transmission 0.92.

The transmission maps (`--dehaze-map`, below) make it visible: on the
vista the map darkens toward the horizon and the leaves are white;
on the portrait the face and hands are the darkest thing in it; on
the interior it is the window. darktable's module does the same, and
its manual says to use it on hazy landscapes. A guard was looked
for and none was found that would leave the vista alone: no
statistic the engine has separates the three (mean transmission
0.92, 0.94, 0.92; the airlight's least channel over its greatest
0.97, 0.81, 0.87, and a warm haze at sunset sits where the portrait
does), and a tighter neutrality floor that caught the skin would
catch that sunset too. So there is no guard, the slider is a
landscape tool, and a mask is how a face is kept out of it when
Dehaze gets a place in the local adjustments. The test
`a_neutral_scene_is_read_as_hazy` records the behavior on a
synthetic low-saturation scene: at +50 its mean luminance drops
22.7%, the worst pixel 33%, and the airlight is its brightest grey.

**Where it runs.** The Dehaze is the DETAIL section's third slider,
under Texture and Clarity: `Detail::dehaze` in the edit, following
the section's switch as they do, and `Detail::dehaze_options()`
beside `Detail::options()`. The worker runs it on the one copy it
makes after the retouch, in the order local contrast, dehaze,
sharpen: the local contrast first so the haze is read off the
picture as the user shaped it, the sharpen last so its contrast
threshold is measured on the picture with its haze gone; the export
takes the same developed picture, so the viewport and the export
agree. The base cache is untouched: `same_base` is unchanged,
`same_develop` compares the whole Detail section, and a slider move
costs those three stages alone. `DevelopSettings::dehaze` exists for
`develop()` callers and runs in `finish` before the sharpen; the
worker and the CLI leave it `None` and run it themselves after the
lens, as they do the local contrast and the sharpen. The CLI has
`--dehaze N` (±100), held through the same `Detail` type the panel
uses, and prints the amount, the strength, the airlight, the grid
factor, the mean and least transmission and the time;
`--dehaze-map PATH` writes the transmission the dehaze applied as an
8-bit grey PNG (black 0 to white 1), the tool the halo work wanted.
The Lightroom import reads `Dehaze` with `Texture` and `Clarity`
into the Detail section, ±100 onto ±1, so a preset carrying Dehaze
alone touches nothing else; the preset test that asserted it
unmapped now asserts the value and the section. The history says
"Dehaze +50" when that is the one thing that moved in the section,
"Detail" when more did, in the section's place in the panel's
order. The field sat at the top level of the edit for a few days of
master before the DETAIL section landed; `migrate` moves a sidecar's
top-level `dehaze.amount` into `detail.dehaze`, within version 3,
since nothing shipped between.

**Tests.** `develop::dehaze`: zero is the identity; a clear
high-contrast synthetic scene barely changes (mean luminance change
under 3%, worst pixel under 10%, mean transmission over 0.95); a
hazy scene (radiance mixed with a clearly colored airlight,
(0.95, 0.85, 0.70), by a transmission that falls across the frame,
with a band of pure airlight for sky) gets its airlight back within
0.03 on each channel, its transmission within 0.05 of
`1 - s + s t` on average away from the sky and the edges, its
contrast back to over 70% of the scene's (79% measured, the share
the strength of 0.8 leaves), and its pixels within 0.03 of the
model's `A + t (J - A) / (1 - s + s t)`; at the full amount on a
neutral scene no channel falls under a fifth of its input, none
goes negative, and the sky band keeps over half its airlight; a
neutral scene at +50 darkens by 10 to 30% (the record above); a
negative amount on the hazy scene lowers contrast; black, clipped
white at 8.0, and a half-and-half frame stay finite and where they
were at ±1 and 0.3; the reduced map on an eightfold copy agrees
with the full-size one within 0.03; the box mean and min filter
handle their edges. The edit crate tests the schema (an old sidecar
without the field reads as zero, one from the top-level days
migrates into the section and writes back there; `same_patched`
holds across a dehaze change and `same_develop` does not; the
section's switch takes the dehaze off with the local contrast) and
the history's rows. The CLI tests
`sharpen(dehaze(lens(x)))` against the pieces run by hand.

**Measured.** 6000x4000, release: the stage takes 0.04 s alone
(0.10 to 0.12 s with the map written), the grid at 1/4, against the
600 ms the sharpen takes; the editor's whole develop with dehaze
and sharpen is 1.0 s on this frame.

**Open.** A sun in the frame will be read as airlight and the sky
left alone, the prior's known failure; a mask or a manual airlight
would answer it. No local Dehaze yet, which is the answer to the
portrait. The neutrality floor of a quarter is a guess that a warm
sunset haze (ratios of 1 : 0.6 : 0.3) clears with room. The hue
divergence along a sunset horizon at +100 would want a second
airlight, which is a different model.
