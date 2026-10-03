# 226. The display curve on a norm, behind a switch (2026-10-02)

The first of the scene-referred tail's four parts. The display curve
is Narkowicz's fit of the ACES output transform with §145's shoulder,
run on each channel on its own. A bright saturated color's channels
reach the shoulder at different points, so the hue turns as it rolls
off: on the reference set a red-orange lamp goes yellow, a blue sky
by the sun goes cyan, and the chroma that is left is whatever the three
channels' clipping leaves. Measured on the rendered output below, the
brightest saturated pixels of a frame turn by a median of 7 degrees and
up to 18.

**The designs read.** darktable's sigmoid has two ways with color. Per
channel, with a "preserve hue" mix that puts the middle channel back
where the input had it between the other two (the Blender filmic
idea): the saturation is the per-channel curve's and the hue held is
RGB's hexcone hue, which is not a perceived one. And "RGB ratio": the
curve on the mean of the channels, one gain on all three, then a
hyperbolic compression of chroma against the display's cube, which
leaves a color far inside it about as it is and takes one at a face or
past it back onto the face; the nearer the mapped mean is to white,
the less room and the less chroma. AgX runs a per-channel curve in a
space whose primaries are pulled in toward white and rotated, so the
per-channel desaturation is gentler and its hue skew is chosen, then
pushes them back out; the skew toward yellow in bright reds is on
purpose. ACES 2.0 tone-maps lightness in its own appearance model and
compresses chroma and gamut there, at constant hue; it is the most
complete of the three and the heaviest, and its model is a whole
module of its own. The RGB ratio design fits what this part asks: a
norm, a held hue, and a step toward white that comes from geometry
rather than from a constant.

**The curve.** `tail::tone_norm`, ported from the sigmoid's
`process_loglogistic_rgb_ratio` (Jakob Dove, GPL-3.0-or-later). The
norm is the mean of the channels, as there. The curve on it is
today's `tone`, not the module's sigmoid, so a neutral is today's
curve exactly: mid grey and everything under it, white at 3.27 stops
over mid grey, and the slope it meets white with, a tenth of a display
stop per scene stop, so the corner is as soft as it was. The module
mixes the RGB toward the neutral by its share; in linear light that is
a straight line in chromaticity, which Oklab reads as a turn, a bright
blue as much as 26 degrees toward violet on its way to white. So the
share is applied in Oklab instead, lightness and chroma both moved
toward the mapped neutral's at the pixel's own hue, and the hue holds
through the whole step. The share is the module's formula rewritten in
reciprocals, `1 / (a + sqrt(a^2 + r^2))` with `a` half of one less the
chroma squared and `r` one over the room: the module's epsilon of a
millionth left a channel at zero lifted by about that, which a steep
point curve made into levels. A color past the black face, which the
chroma below can make, is put on it.

**The chroma it gave back.** One gain on three channels keeps the
scene's chroma, where the curve per channel multiplies a color's
departure from grey by its slope in stops: 1.1 at mid grey, up to 1.6
two stops under, and over grey falling toward a tenth at white.
Without that the first version left the reference set's mid-tones at
70 percent of today's chroma, a median over every frame, and a skin
tone at two thirds of it two stops down: the picture read as faded
and the comparison would have been about that. So the chroma is
scaled in Oklab by the curve's slope (`tone_slope`, the fit's and the
shoulder gain's derivative), both ways, which is the per-channel
curve's own saturation to first order, at a held hue. A first version
took only the slope over one and kept all of the chroma where it is
under; a review's measurement of it found bright skies and skin
highlights 20 to 30 percent more chromatic than today between
lightness 0.75 and 0.95, then collapsing near white. Scaled in full,
the slope pushed a dark saturated orange past the black face and the
clip there turned it five degrees, so it is weighed by the color's
lowest channel over the mean: whole near grey, nothing for a pure
primary, whose channels the per-channel curve does not spread either.
The weight is a square root with a floor,
`(sqrt(t + 0.01) - 0.1) / (sqrt(1.01) - 0.1)` of the share `t`: zero
at zero and one at one, with a slope of about 5.5 at zero. A plain
square root, tried first, has no bound on its slope there, which is
where a saturated color's lowest channel sits: the curve moved 0.6
levels for a nudge of 1e-5 where the curve per channel moves 0.04, and
the random parity test failed seed 16 by 4.8 levels on it. A linear
weight, tried next, moves 0.07 for the nudge but leaves the shadows'
chroma a tenth under today's (0.90 and 0.92 in the two lowest bands);
the floored root moves 0.16 and keeps them within five percent.
`min(4t, 1)` came nearest one under lightness 0.75 but overshoots
today's chroma in the shadows (90th percentiles of 1.06 to 1.10) and
drops the 0.85 to 0.95 band to 0.86; no band over today's was the
preference, and the floored root's two bright bands sit two or three
percent over it, on the median frame.

The chroma on the norm over the chroma per channel, by Oklab lightness
per channel, over the 56 frames (pixels with chroma over 0.02 per
channel; a band counted on a frame with 200 such pixels in it):

| lightness | frames | median of the frames' medians | lowest frame | highest frame | highest frame's 90th percentile |
| --- | --- | --- | --- | --- | --- |
| under 0.35 | 56 | 0.95 | 0.89 | 1.00 | 1.03 |
| 0.35 to 0.55 | 54 | 0.97 | 0.83 | 0.98 | 1.02 |
| 0.55 to 0.75 | 54 | 1.00 | 0.94 | 1.05 | 1.07 |
| 0.75 to 0.85 | 52 | 1.03 | 0.72 | 1.07 | 1.10 |
| 0.85 to 0.95 | 42 | 1.02 | 0.26 | 1.15 | 1.20 |
| 0.95 and over | 13 | 0.64 | 0.20 | 1.22 | 1.59 |

With the slope taken over one only, the bands from 0.75 up were 1.20,
1.31 and 0.50 (the review's figures). The top band is the least sure
of these: what lands in it depends on which pixels are taken, and only
a dozen or so frames have 200 saturated pixels that bright. The low
frames in the top bands are the lanterns and the headland, where the
step has the chroma (below).

**The switch.** A field on the edit, `display_curve`, `channels` or
`norm`, where the demosaic is the precedent for a per-picture choice
of method: a picture is compared one way or the other, and the export
reads it from the same edit the viewport does. Left out of the sidecar
at its default, so a picture nobody switched writes what it always
did. The finish reads it through `Baked::global`, the viewport through
`View::with_look` (the shader's curve mode, 2 for the norm), the
droppers through `pick`; nothing else builds either. A toggle at the
foot of the LIGHT section, "Hold hue to white", shown on a raw's
global look only, the history's step named the same, and `--hold-hue`
on the command line for a comparison from a script, on the first file
only as `--exposure` is. Off by default: no picture changes until it is
thrown.

A preset, a sync or a copy of the Light section does not carry it.
The schema cannot tell a preset made before the switch from one made
from a picture per channel, since the field is left out at its default
in both, and carrying it would turn the switch off on every picture
such a preset is laid on, every Lightroom import among them. The
switch is the picture's, as the comparison wants it; a set is compared
with `--hold-hue` or by throwing it per frame. A Section of its own
would let a preset carry it on purpose, if a later step makes that
worth a line on the preset sheet.

**Checked.** Tests: `a_neutral_through_the_norm_is_todays_curve`
sweeps a neutral from eight stops under grey to past white against
`tone` (four millionths), white at the same scene value and the slope
there within a thousandth of a display stop;
`a_saturated_ramp_keeps_its_hue_and_steps_toward_white` takes the
primaries, the secondaries, an orange, a sky and a skin from six stops
under to eight over, the Oklab hue within a tenth of a degree all the
way, the step monotone once at the white face, white at the end, and
clipped to the cube still within 0.003 across the a, b plane;
`a_color_near_grey_keeps_the_chroma_of_the_curve_per_channel`,
`the_slope_is_the_curves`, `the_chroma_kept_is_darktables` (the
module's formula with its epsilons, in double, against ours);
`the_norm_curve_on_the_gpu_is_the_cpus` and the random parity test,
which now draws the switch half the time, as a part of its own
("display curve"), drawn last so every earlier draw is what it was.
RADV found one thing the other two did not: the Oklab round trip
rounds a channel at zero to a billionth either side, differently on
the GPU, and a local's steep point curve made that 2.8 levels. The
step is now added to the color as the difference of two round trips
that round alike, so a color it leaves alone comes back exactly, and
what is left on a zero channel is held to zero by the Oklab pass's own
`snap_residue`. The shader's cap on the share kept is a `select` at a
zero room, not an infinity left to `min`.

With the linear weight, seeds 0 to 32 on NVIDIA and lavapipe, with
the switch drawn and with it skipped
(`GREYCARD_PARITY_SKIP="display curve"`, which is master's draws).
Skipped, what fails is the test's and master's: seed 16 edit 43 (3.9
levels on NVIDIA, the steep cap on both), 23 edit 69 and 32 edit 368
by the steep cap on both, and 29 edit 51 at 7.7 levels on lavapipe;
none of them names the switch. Drawn, 29 and 32 passed, their failing
edits drawing the norm, and 23 failed as before.

With the floored root, seeds 1 to 10 pass on both drivers with the
switch drawn, at most 1.08 levels on NVIDIA and 1.64 on lavapipe.
Seed 16 edit 43 fails the steep cap on both (15.2 percent) and on
lavapipe by 3.9 levels as well, 1.5 on NVIDIA; skipped, the same edit
is 0.6 levels on lavapipe and still over the cap, so the levels are
the switch's: a pixel with a channel at zero, where the root's slope
of 5.5 is, under steep point and color curves. Seed 12 edit 28 fails
by the steep cap alone, 16.6 percent of its pixels steep against 15,
with the GPU within 0.6 levels on both drivers; without the switch
that edit is 10.7 percent steep. It is a whites of +2 under a contrast
of 0.5, which puts much of the frame at the white face: there the
curve per channel has clipped a channel and is flat, where the norm's
step is still grading the chroma by how near the brightest channel is
to white, so more pixels answer a nudge. A smoothstep weight did not
change it (17.1 percent). The cap is the test's guard against a broad
break hiding as steepness; this is not one, and the cap is left where
it is.

**Measured.** Every raw in the test folder, 56 frames, exported at
1200 pixels with nothing done to them, three ways: per channel, on the
norm, and on the norm three stops down, which no frame's roll-off
reaches and which gives each pixel its scene hue. The pixels taken are
the tenth brightest (at least 200) of those saturated in the scene, an
Oklab chroma of 0.04 at three stops down, and at 0.85 of white or over
on screen; a hue is read only where the output has chroma above 0.02.
On pixels neutral in the scene, an Oklab chroma under 0.004 at three
stops down, the 99th percentile of the difference is 0.65 of an 8-bit
level on the median frame and 0.73 on the worst; under 0.01 it is 2.2
and 2.8, since a nearly neutral pixel's small chroma is scaled by the
slope. (The draft before this said 0.0066 in lightness was under a
level; near mid grey it is about two.) On the 36 frames with
enough such pixels, the median turn is 7.3 degrees per channel and 1.0
on the norm, the frames' medians' median. The norm's figure is near
zero by construction, since the reference hue is the norm's own curve
three stops down; what it shows is that the roll-off adds no turn to
the one the curve already holds, and the per-channel figure is the
measure of what the switch removes. The frames where it was
worst, per channel against the norm, sRGB output and then Rec.2020:

| frame | per channel | norm | per channel, Rec.2020 | norm, Rec.2020 |
| --- | --- | --- | --- | --- |
| lanterns at night | 17.9 | 8.4 | 10.9 | 1.1 |
| jets against an overcast sky | 16.5 | 9.4 | 12.5 | 0.5 |
| a headland, the sun in frame | 17.1 | 0.9 | 12.0 | 1.5 |
| a couple under wisteria, backlit | 18.0 | 2.3 | 14.6 | 1.5 |
| a portrait in an alley, neon | 9.5 | 4.7 | 8.2 | 1.0 |
| a city under a clear sky | 8.4 | 1.3 | 8.3 | 2.0 |

What is left in sRGB is the output's clamp, not the curve: the same
frames exported in Rec.2020 turn by one to two degrees on the norm.
The jets' blue and the lanterns' red are outside sRGB, and the clamp
at the output matrix takes each channel to the edge on its own, which
turns them. That is part three's.

**What it looks like.** Most frames are hard to tell apart away from
their highlights. A sky by the sun stays blue as it brightens to white
where per channel it went cyan. The lanterns are where the call is.
Per channel, a red-orange lamp's red and green clip and it burns
yellow and saturated to its core. On the norm, held at its hue, it
goes toward white through a pale salmon and gets there sooner: its
brightest pixels keep about a fifth of the chroma per channel keeps,
and the lanterns read as white paper with a pink cast rather than lit
from inside. That collapse is the step's, not the slope's. The mean
norm takes a saturated color to white when its mean reaches display
white, and the hyperbola starts taking its chroma about a stop and a
half before that, where per channel keeps a chroma of about 0.1 until
its weakest channel clips, a stop later. With no slope scale at all
the top two bands measure 0.217 and 0.263 of today's chroma on the
lanterns. That yellowing is what AgX keeps on purpose and what reads
as fire; holding the hue in Oklab is what the eye calls the same hue,
and is not what it expects of a flame. On this frame, as it stands,
per channel looks more like the scene.

**The step has no control yet.** Nothing sets where the compression
toward white starts or how fast it goes: both fall out of the mean
and the cube, and the mean reaches white late for a saturated color
and lets the hyperbola start early. Before the switch could be on by
default the step needs one of a start point of its own, a room
measured toward the brightest channel rather than from the mean, or a
chosen skew toward yellow for a bright red; the roadmap carries the
skew as the feel track's call.

**Lightness of a saturated color.** The mean is a poor brightness for a
pure primary: a pure Rec.2020 green is a third of its mean's channel
sum where its luminance is two thirds, so the toe darkens it, by 0.07
in Oklab lightness a stop under grey against per channel, and a pure
blue comes out 0.1 lighter. Filmic's power norm, the sum of cubes over
the sum of squares, kept every test color within 0.03 of per channel
in a sketch, but a norm that sits at the brightest channel leaves a
saturated color inside the cube to the end, so the step toward white
would need a rule of its own rather than the cube's geometry. On real
frames the mean is within 0.02 for skin, sky and foliage; the primaries
are where it shows.

**For the other three parts.** (2) The point curves run after this on
encoded values per channel, so the master curve still skews a hue this
now holds; on a norm it loses the per-channel curve's saturation for
the same reason this did, and the same slope, the curve's own, is what
would put it back. (3) Everything left in sRGB is the output clamp: a
gamut compression at constant hue in Oklab after the curves and the
look table would take the lanterns from 8.4 degrees to about one. The
step here leaves the cube by at most two hundredths at the white face,
on a bright magenta and cyan, and by under a thousandth at the black
face; the compression would take that too. (4) The clamp before the
curves is what trims those today; dropping it means the curves and the
look table accept values a hair outside the unit range.

**Cost.** The shader does three Oklab round trips more per pixel with
the switch on, and it is not timed. The review measured the CPU side
at 6.8 to 68 nanoseconds a pixel single-threaded, about 0.2 seconds on
a 100-megapixel export over 32 threads. Lavapipe and an integrated GPU
are the viewport's cases to time before the switch's default changes.
