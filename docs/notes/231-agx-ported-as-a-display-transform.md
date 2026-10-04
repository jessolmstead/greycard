# 231. AgX ported as a display transform: Blender's formation with Punchy, behind the switch (2026-10-03)

The first item of §230's call, with one change of course on the way:
the port is Blender's wide-gamut AgX (Eary Chow's formation), not
Sobotka's sRGB-only config that §227 and §230 judged. AgX with the
"AgX - Punchy" look is the third value of the display-curve switch
§226 added, beside per channel and Hold hue to white, on the CPU and
in the shader, checked against OpenColorIO's render of Blender's own
config. Per channel stays the default; no picture changes until it is
switched.

**Why Blender's and not Sobotka's.** The first port was Sobotka's
config as it stands: linear BT.709 in, a clamp at zero, his inset (a
compression of a fifth, no rotation, no outset), a 16.5-stop log and
one sigmoid, Punchy a CDL (power 1.35, saturation 1.4). It met OCIO to
a hundredth of a level, and it is in the branch's history. But the
pipeline is built for wide gamut, and that transform zeroes the
negative BT.709 channel of every color outside sRGB on the way in: the
lanterns' red, the jets' blue, the bluest Fuji frames (§226 measured
them outside sRGB) are moved along one axis rather than toward white,
which is the fault AgX exists to avoid, and it renders the same BT.709
cube for every output. Blender's formation works in Rec.2020 with a
guard rail in place of the clamp, forms a different image for each
display gamut, and is what darktable's module parametrizes as its
defaults. So the port is that, and the gain below makes the two modes
compare as a rendering.

**What Blender's AgX is, as ported.** From the generator
(github.com/EaryChow/AgX_LUT_Gen, on Sobotka's AgX-S2O3), on linear
Rec.2020:

1. The lower guard rail (`compensate_low_side`, Eary Chow's method,
   the same math as darktable's `_compress_into_gamut`, which is what
   is ported): a color with a channel under zero is offset until that
   channel is zero, then scaled so its luminance, corrected for the
   negative part by the opponent color, is what it was. A color inside
   Rec.2020 is untouched. The luminance weights are darktable's
   (0.26582, 0.59847, 0.13571), which are the set Blender's shipped
   LUTs were made with; the generator's later set (0.2589, 0.6105,
   0.1306) matches neither: with it the Rec.2020 LUT's nodes outside
   gamut are off by up to 7.8e-3 and the sRGB LUT's by 17.8 levels,
   with darktable's by 7e-8 and 1.2e-6.
2. The inset matrix: the Rec.2020 primaries rotated by [2.14, -1.23,
   -3.05] degrees and inset by [0.3297, 0.2805, 0.1248] toward white,
   in chromaticity, as the config's "AgX Log" writes it. (The LUT
   files' comments give BT.709-derived numbers, rotate [3, -1, -2] and
   inset [0.4, 0.22, 0.13]; the generator says those were a mistake and
   gives the Rec.2020 ones that make the same matrix.)
3. The HSV hue, remembered.
4. A log from ten stops under mid grey to the white relative exposure
   over it (Blender's is 6.5; ours is set below), and Jed Smith's
   sigmoid per channel: pivot at mid grey's place in the range and an
   encoded 0.18^(1/2.4), slope 2.4, toe and shoulder powers 1.5,
   scaled to pass through (0, 0) and (1, 1) and continuing past one.
   The output is a 2.4-encoded value; decoded by 2.4.
5. The hue mix: the HSV hue after the curve moved 60 percent of the way
   back to the hue before it, so 40 percent of the per-channel shift is
   kept, which the generator calls the flourish kept in control.
6. The outset matrix, the primaries pushed back out by [0.3232, 0.2833,
   0.0374] with no rotation (the generator derives it from the same
   construction; we reproduce the inset from its parameters to 1e-8
   and take the outset from the same code), and a clip to the unit
   cube: display-linear Rec.2020.
7. For a display narrower than Rec.2020, a second guard rail in the
   display's space, the luminance read in Rec.2020; Blender's sRGB
   generator further lerps that luminance toward the opponent-
   compensated one by its 0.08 power, a step darktable does not have
   and this port does not take (below); then the display's encoding.

Blender ships the whole of 1 to 7 as a 57-cubed LUT per display,
indexed by FilmLight E-Gamut log over 25 stops. The look "AgX -
Punchy" is not Sobotka's CDL: in the config it is a shadows tone grade
(OCIO's `GradingToneTransform`, shadows 0.2 on each channel and 0.35
on the master, start 0.4, pivot 0.1) and a power of 1.0912, both in the
25-stop "AgX Log" of the inset space, and OCIO runs the look in that
space and the formation after it, compensation and inset again.

**Where it came from.** darktable's AgX module (`src/iop/agx.c`, Kofa,
2025, GPL-3.0-or-later) is this formation with the inset, rotations,
outset and the curve's shape as parameters, and its defaults are
Blender's: red, green and blue insets 0.2946, 0.2586, 0.1464 with
rotations 0.0354, -0.0211, -0.0631 radians (the numbers that make
Blender's matrix under darktable's D50 handling), outsets 0.2908,
0.2632, 0.0458 with no unrotation, the curve's toe and shoulder 1.5,
gamma 2.4, contrast 2.4 (set through its gamma compensation), and the
"preserve hue" mix at 0.6, which is Blender's 40 percent kept. Its
`_compress_into_gamut` is the lower guard rail. The port takes from it
the rail, the log encoding, the sigmoid with its scales, the decode and
the hue mix, as `agx.rs` says; the matrices are taken as numbers (the
config's inset, the construction's outset) rather than rebuilt from the
parameters, which is the feel track's to open. The shadows grade is
ported from OCIO's `FauxCubicFwdEval` and `ComputeHSFwd` (BSD-3-Clause;
the license's notice, conditions and disclaimer are kept in `agx.rs`'s
header, which the shader's header points at), the forward case only,
which is the look's. The guard rails descend from one function,
darktable's GPL `_compress_into_gamut`, which is Eary Chow's luminance
compensation for Rec.2020 as darktable ported it; Eary Chow is named for
the method, and his repository, which carries no license, is not a
source of any code here. The lower rail is that function with its
weights; the output rail for a narrower display is the same function
applied in the output space, its weights Rec.2020's folded through the
output's matrix. The first version of the output rail followed Eary
Chow's sRGB script statement by statement (a lerp of the luminance
toward the opponent-compensated one by its 0.08 power, on both sides);
the review found it, and it is gone. What that step's absence costs is
below. Not ported: darktable's linear section between toe and shoulder
(zero length here), its fallback power curves for a slope too shallow (a
debug assertion says when they would be wanted), and its exposure
pickers.

**Which space the inset is applied in.** Blender's LUT is indexed by
E-Gamut log, but inside it the generator decodes to linear Rec.2020
first and applies the inset there; E-Gamut is only the LUT's input
encoding, chosen to cover more than Rec.2020. darktable applies the
inset in its base profile, Rec.2020 by default. Our working space is
Rec.2020, so the inset is applied where both apply it, with no matrix
in or out, and the result is the same transform; the only departure is
in the LUT's input conversion, which our port does not need.

**The output side.** Blender forms a different LUT per display: the
sRGB and P3 ones add the second guard rail in the display's space
(step 7); the Rec.2020 one has none, its formation clipped to the
Rec.2020 cube instead (a Rec.2020 red at mid grey comes out of the
outset at [0.17, -1e-4, -5.7e-3] before that clip). The oracle decided
whether we need it: OCIO's
sRGB view against the Rec.2020 formation with a plain BT.709 clip is
off by a median of 1.5 and up to 113 levels on the primaries and 81 on
a ring of colors outside sRGB; with the rail, 0.2 and 26, and 0.12 and
8 on the ring, which is the LUT's own interpolation error (below). So
the finish's output stage applies the rail in AgX mode for any output
space, with the luminance weights folded from that space's matrix back
to Rec.2020 (`agx::output_rail`, `rail_weights_for`); for a Rec.2020
export the weights are Rec.2020's and the rail never acts. The shader
carries the three weights in the spare lane of the output matrix's
rows. Per channel and the norm keep the plain clip. This compresses to
the output's gamut for every output, as Blender does: what BT.709
cannot hold is brought to its face at held luminance rather than cut
per channel, and a Rec.2020 export keeps what Rec.2020 holds. Letting a
narrower export keep more than its gamut is not a thing an export can
do.

Two departures are measured at the full grid of Blender's sRGB LUT,
the 50 630 in-range nodes (the reviewer's scan, where the LUT is the
generator's arithmetic and not an interpolation), against the node's
value decoded and re-encoded for the file, in 8-bit levels:

| output rail | nodes inside Rec.2020: over a level, p99, max | nodes outside Rec.2020: over a level, p99, max |
| --- | --- | --- |
| with Blender's lerp (darktable's weights, the review's second pass) | 2.4 percent, 2.1, 18.1 | 12.7 percent, 13.3, 22.0 |
| darktable's form (shipped) | 29.3 percent, 17.7, 20.4 | 37.5 percent, 18.7, 21.9 |

The first row is the early clip alone, and it is the clip and not the
weights: Blender's sRGB and P3 generators take the formation to the
display's space unclipped, rail it there and clip after; our chain clips
the formation to the Rec.2020 cube first, because the point curves and
the look table sit between the curve and the output matrix and expect
the cube (§226's tail, part (4), is the item that drops that clip). The
worst inside node is a saturated yellow-green ([0.070, 0.257, 0.0018]
linear) whose formation blue goes negative and is clipped, so its red
reads 0 against the LUT's 18. The Rec.2020 LUT at the same grid is 0.23
inside and 0.03 outside. The second row is what the lerp's absence costs
on top of that, and it is not small: the 99th percentile inside Rec.2020
goes from two levels to eighteen. The branch is left at darktable's form
all the same, since the lerp is Eary Chow's and his script has no
license; the alternative, asking him for one, was put to the user, who
looked at the saturated frames under darktable's form and accepted it;
the roadmap keeps the license as the way back to Blender's table. The
fixture carries 200 nodes of the sRGB LUT inside Rec.2020 and 200
outside, and `the_srgb_formation_at_the_luts_nodes` pins the shipped
figures there: inside 30 percent over a level and 19.5 at most, outside
36 percent and 20.4, held under 40 and 50 percent and under 25 and 30
levels. The same nodes would have caught the weights.

**A color outside Rec.2020.** The camera matrix can produce one. The
lower guard rail takes it: offset onto the Rec.2020 face at held
luminance, the hue in RGB terms a straight line toward white, before the
inset; nothing is zeroed. With darktable's weights the LUT's grid nodes
outside Rec.2020 agree with the formation as we evaluate it to 7e-8, as
the nodes inside do; the 1.1e-2 a first run saw there was the
generator's later weights, not the rail.

**Where it joins the pipeline.** `finish::tone` leaves display-linear
working-space values, and the point curves, the look table, the output
matrix and the encoding follow. The formation's output is
display-linear Rec.2020 already, so it joins there with nothing to
decode; the output rail sits where the clip was. The baseline exposure
and the Light sliders act before it as before any curve.

**Mid grey and white, the two points that set it to this pipeline.**
Blender's base puts scene mid grey at 0.18 display-linear (a test
holds the formation to it to a ten-thousandth), and Punchy takes that
to 0.097, where the curve per channel puts 0.18 at 0.267: 1.46 display
stops under. A tester throwing the switch would have seen that, and
§230's judging never did, since the tool matched median luminance by
exposure. §226's rule for
the norm was that a neutral is today's curve exactly: the switch
compares a rendering, not a brightness. So the AgX mode applies a
fixed gain to the scene-linear values after the baseline and the Light
sliders, inside the mode only, chosen so that mid grey through AgX
Punchy comes out at the display-linear value `tone` gives it, found
by bisection on `tone(0.18)` at construction.

White is the second point, and the review found it. With Blender's
white relative exposure of 6.5 stops, a neutral at the sensor's clip
plus the baseline (`DISPLAY_WHITE`, 1.736, 3.27 stops over mid grey)
rendered at 0.759 display-linear, 226 of 255, where per channel puts
it at 255: on the backlit couple and on a snow frame every clipped
pixel sat at 226 and none reached 250, the snow's brightest down from
236 to 207. Blender's own view at the same matched mid grey gives
225.7, so it is the design, a flat grey clip, and §145 fixed exactly
that at 242. So the white relative exposure, darktable's
`range_white_relative_ev` and what its auto-white picker sets, is set
for this pipeline: the largest value at which a neutral at
`DISPLAY_WHITE` still reaches display white, found by bisection with
the mid-grey gain found inside each step, since the pivot moves with
the range and the gain with the pivot. The result: **white relative
exposure 3.872 stops over mid grey (Blender's 6.5), gain 2^1.302 =
2.466 (+1.30 stops)**. The black side stays where Blender has it, ten
stops under mid grey. A neutral through the finish with the baseline
in, 8-bit sRGB:

| stops over mid grey | before (white 6.5) | after (white 3.87) | per channel |
| --- | --- | --- | --- |
| +2.47, the sensor's clip | 226 | 255 | 255 |
| +4 | 245 | 255 | 255 |
| +5 | 253 | 255 | 255 |

`mid_grey_through_agx_is_per_channels` pins mid grey through
`Source::curve` in AgX mode to per channel's within a thousandth of a
display stop and holds the shader's five literals (the gain, the
range, the pivot, the two scales) to the CPU's;
`a_neutral_ramp_through_agx_rises_to_white` finds white at 3.270
stops, per channel's to a hundredth, and prints the table. The look
this costs is measured below. What mid grey should be at all, the
baseline measured against the camera JPEG (§141, §143), is left for
the default flip; this change decides only that the two modes agree
on it and on white.

**The switch.** `display_curve` on the edit gains `agx` beside
`channels` and `norm`, left out of the sidecar at its default as
before. The LIGHT section's toggle is a three-way choice, "Display
curve: Per channel, Hold hue to white, AgX", named by
`DisplayCurve::name`; the history step is "Display curve: AgX";
`--agx` on the command line beside `--hold-hue`, first file only.
Presets, sync and copy do not carry it, for §226's reason. The
viewport (`View::with_look`, curve mode 3), the export
(`Baked::global`) and the droppers (`pick`) read it through the one
`Source::curve`; nothing else builds a curve. A camera match table
fitted under per channel (§180, §181) is still applied under AgX as
it stands; the tag that records the transform a table was fitted
under, and the Look section's refit on a mismatch, is §230's roadmap
item and waits on this.

**The oracle.** `tools/agx-oracle.py` writes a fixture
(`crates/greycard-ui/tests/fixtures/agx-oracle.txt`, 88 KB of synthetic
numbers, no raw data). Three kinds of line. 200 grid nodes of Blender's
Rec.2020 formation LUT whose input is inside Rec.2020 and within the
curve's range, with the value the LUT holds: at a node the LUT is the
generator's own arithmetic and not an interpolation of it, and
`the_formation_is_blenders_at_the_luts_nodes` holds our formation there
to 2e-6 of the 2.4-encoded value (the reviewer ran the generator itself,
Eary Chow's scripts through colour-science, at the sweep's inputs: the
formation inside Rec.2020 agrees to 5.9e-8, the Punchy grade to 1.3e-4
relative against OCIO's ops without its LUT). 400 nodes of the sRGB LUT,
above. And OCIO's renders of Blender's config from "Linear Rec.2020":
the view "AgX" on the sRGB display without and with the look "AgX -
Punchy", and on the Rec.2020 display with it, over a neutral from ten
stops under mid grey to six over at a quarter stop, the Rec.2020
primaries and secondaries, a skin, a sky and a foliage color at half a
stop, and a ring of twelve saturated Rec.2020 colors outside sRGB at a
stop; 566 colors. Between nodes OCIO interpolates the 57-cubed LUT
tetrahedrally (its cells are 0.45 stops across), and where the formation
has a corner, the clip after the outset on a saturated color and the
sRGB rail, the interpolation lifts a channel at zero by whole levels; so
the sweep's figures are the LUT's own error, not the port's.
`agx_punchy_is_opencolorios`, our chain with the gain taken off the
input (the mid-grey test pins the gain itself), the output rail and the
export's encoding, against Blender's own transform (white 6.5, no gain,
`Agx::blender`), in 8-bit levels, the max over channels:

| view | sweep | median | 90th percentile | max |
| --- | --- | --- | --- | --- |
| sRGB, Punchy | neutral ramp | 0.025 | 0.15 | 0.22 |
| sRGB, Punchy | skin, sky, foliage | 0.09 | 2.2 | 12.0 |
| sRGB, Punchy | ring outside sRGB | 0.14 | 1.1 | 10.4 |
| sRGB, Punchy | primaries and secondaries | 0.33 | 6.8 | 55.8 |
| sRGB, base | neutral ramp | 0.05 | 0.15 | 0.23 |
| sRGB, base | skin, sky, foliage | 0.15 | 5.3 | 11.5 |
| sRGB, base | ring outside sRGB | 0.13 | 1.0 | 8.1 |
| sRGB, base | primaries and secondaries | 0.40 | 4.2 | 26.2 |
| Rec.2020, Punchy | neutral ramp | 0.03 | 0.14 | 0.22 |
| Rec.2020, Punchy | skin, sky, foliage | 0.11 | 0.37 | 3.4 |
| Rec.2020, Punchy | ring outside sRGB | 0.15 | 0.59 | 4.1 |
| Rec.2020, Punchy | primaries and secondaries | 0.60 | 11.8 | 27.9 |

The neutral ramp and the medians are the interpolation's ordinary
error and the maxima sit on the corners: the sky leaves sRGB at four
stops over, and a primary's zero channels are lifted by the grid. The
55.8 on the sRGB Punchy primaries is not even the formation LUT's: for
the look, OCIO runs the process space through the 37-cubed
`luminance_compensation_bt2020.cube`, and that coarser table's
interpolation lifts the zero channels; against the look built from its
ops without that LUT the port's largest is 27.9, the same as the
Rec.2020 view's. The test's bounds are those maxima with a little over
them. A neutral through AgX rises all the way, is display white from
3.27 stops over mid grey on and black at black
(`a_neutral_ramp_through_agx_rises_to_white`); the rails leave a color
inside their gamut exactly alone (`the_rails_touch_only_what_is_outside`).

**On the GPU.** `the_agx_curve_on_the_gpu_is_the_cpus`, the norm's test
generalized to a curve, over the parity frame at seven exposures: at
most 0.0023 against the CPU on NVIDIA and 0.0020 on lavapipe, and over
every channel not at either end a mean signed difference of +0.010
levels (lavapipe -0.001) at an rms of 0.290, which is the 8-bit read
back's own quantization (a twelfth's root is 0.289), so the shader's
curve has no bias the quantization could hide. A second test,
`a_display_curves_gap_before_the_point_curves`, uses a master point
curve rising across a fiftieth of the range as a magnifier, at five
places and three exposures, the output in Rec.2020 so the level is the
curve's own value, and inverts the curve on the GPU's level to recover
each pixel's encoded value before the point curves to a few
hundred-thousandths; the three curves are the same, rms 0.0066, 0.0060
and 0.0058 levels and largest 0.0180, 0.0184 and 0.0184 (lavapipe
0.0178, 0.0178, 0.0175), the chain's float rounding, and AgX adds none
of its own; each is held to a tenth of a level. The random parity test
draws AgX a third of the time, after the norm's draw so every earlier
draw is what it was (a third each, then). Seeds 1 to 10, NVIDIA and
lavapipe: all twenty runs pass, at most 1.28 levels (NVIDIA seed 4,
edit 372, 11 percent steep) and 0.93 on lavapipe; the cap and the
tolerance are untouched. (The Sobotka port that preceded this one
failed seed 2 edit 198 at 2.6 levels on both drivers, a blue pixel on
a master curve stepping 0 to 1 over 1.4 percent of the range, slope
74, where the chain's gap of a ten-thousandth before the point curves
is whole levels after them, §226's sensitivity on seeds 12 and 16; the
pixel lands elsewhere under this transform.)

**Real frames.** `tools/compare-transforms.py` gained an `agx-port`
column, greycard's own export with `--agx`, two Blender columns
(`agx-blender`, `agx-blender-punchy`: the view "AgX" on the sRGB
display without and with the look, exposure-matched as the other OCIO
columns are) beside Sobotka's under their old names, and
`agx-blender-punchy-same`, Blender's Punchy at the export's own
exposure (the baseline of 0.8 stops and the mode's gain of 1.41,
nothing matched), the render the port is checked against; Blender's
sRGB display encodes piecewise, as the export does, so the two files
compare as written. The two are not one pipeline (the export applies
the lens profile and the camera's crop and resizes with its output
sharpen; the linear develop does none of that), so the script compares
twice, over a central 60 percent crop of each resampled to the same
320-pixel grid (an upper bound, with the misalignment in it) and
between their per-channel quantiles at the 1st to 99th percentiles
(blind to alignment). Over all 72 raws of the test folder (the two
Nikon High Efficiency NEFs left out), in 8-bit levels:

| measure | median frame | 90th percentile frame | worst frame |
| --- | --- | --- | --- |
| quantiles, median | 1.0 | 2.0 | 3.0 (a sky frame) |
| quantiles, 99th percentile | 2.0 | 5.0 | 8.0 (a headland under an overcast sky, two sky frames) |
| center crop, median | 1.8 | 3.5 | 7.5 |
| center crop, 99th percentile | 25 | 39 | 59 |

The quantile figures are the port's agreement with OCIO on real scenes,
one to two levels with JPEG quantization at quality 92 in both files and
the LUT's interpolation in OCIO's; the center crop's 99th percentiles
are the misalignment and the sharpening. On the way a first run showed
the export 25 levels under the chain on every frame: the release binary
the run used predated the gain, which the run's export-against-chain
check found (a pixel-for-pixel run of `finish_pixel` over the CLI's
develop at the export's size agrees with the fresh export to 0.4 levels
mean and with the numpy model of the generator to 0.05 levels max), and
the run was redone. These are the figures before the white was moved,
the port against Blender's config as shipped. The white fix changes the
look, measured the same way, the 72 frames' `agx-port` after against
before (the shipped white 6.5): the quantile difference's median is 9.5
levels on the median frame (90th percentile frame 11, worst 15 on the
teal wall), its 99th percentile 19 (29 at most); signed, the median
quantile goes down by 7 and the 90th up by 10: the shorter range at the
same slope makes every stop about 19 percent steeper, so the shadows go
down 8 to 11 levels (skin two stops under mid grey from 46 to 36, a sky
from 42 to 33) and the highlights reach white. Pixels with any channel
at 250 or over: on the backlit couple 7.5 percent where none was (5.9
with all three channels there); on the chalk headland in the sun 17.5
percent where none was (7.8 on all three); the lanterns 4.7 from 1.0;
the greenhouse portrait none either way; the snow frame has no clipped
pixels under either, its snow's 99th percentile 223 against per
channel's 232. By eye on the headland the sky is blue to the horizon
where it was a flat grey, and the sunlit chalk is white. The standouts
of §230's two sets are re-rendered with it into the two compare folders
as `NAME-agx-port.jpg`, 18 and 15 frames, for the user's look. The black
side is left at Blender's ten stops under mid grey; nothing in the tests
asked for it to move.

**A second construction, kept for comparison.** The 19 percent
is a look call, so there is a second construction beside the first:
the slope scaled by the range over Blender's 16.5 stops, so Blender's
contrast per stop is kept with white still at the clip
(`Agx::blender_punchy_soft`): slope 2.03, white relative exposure
3.97 stops, gain 2^1.393 = 2.626. It is behind no user control; the
`GREYCARD_AGX_SOFT` variable picks it for the comparison renders only,
and the shader's literals are the first construction's. The same
standouts are rendered with it as `NAME-agx-port2.jpg` beside the
first's, in the same two folders. Over the 72 test-folder develops
(the port's chain on the CLI's linear develop, no lens, no sharpen, no
JPEG, against OCIO's Blender Punchy at each construction's gain, by
quantiles):

| construction | against Blender's view, median quantile | its 99th percentile |
| --- | --- | --- |
| first (Blender's slope 2.4, white 3.87, gain +1.30) | 7.2 on the median frame, 8.4 at the 90th, 12.0 worst | 21.7, 30.9, 30.9 |
| second (slope 2.03, white 3.97, gain +1.39) | 0.3, 1.2, 5.1 | 14.8, 29.4, 29.5 |
| second against first | 6.7, 9.5, 15.5 | 10.6, 13.9, 20.7 |

The second keeps Blender's tones through the quantiles' middle (a
third of a level on the median frame) and parts from it only at the
top, where white is reached at the clip rather than Blender's 6.5
stops; the first differs from Blender's at every quantile by the 19
percent.

The user looked at both on the 33 standout frames of §230's two
sets, beside per channel and the Blender render that was judged: the
port read as "the best of the three on most frames", the dusk
lanterns a little dark under the first construction; and the two
constructions "very close, trading minor wins across frames", with no
overall winner. So the first construction ships as the default, since
the shader carries its literals, and the second stays in the code as
the feel track's starting point for a contrast and white control; the
lanterns' shadows are the Light sliders' to lift. The rail at
darktable's form was looked at on the saturated frames (the lanterns,
the neon, the bluest frames, the orchid) and accepted: "saturated
colors seem pretty fine". Looked at side
by side, the three frames asked for are the same picture as OCIO's:
the lanterns at night (quantile median 1, p99 5), a portrait in a
greenhouse (1, 1) and the bluest frame, a teal fur wall with a neon
sign and a figure in pale blue (the scan of the develops for color
outside BT.709 puts a hanging yellow orchid first at 18 percent of its
pixels, the lanterns second at 13, the teal wall far down at a quarter
of a percent; the teal is inside BT.709 and only looks the bluest). Of
the orchid, the lanterns and the teal wall: the orchid's greens and
yellows render full and clean, the teal wall and the neon are held
without the magenta skew per channel gives the sign, and the lanterns
are the change to see below.

**The judged look carries over, measured.** §230's judging was on
Sobotka's config. The tool now renders both, exposure-matched to the
per-channel export, and compares Sobotka's Punchy (re-encoded for
sRGB) with Blender's by the same quantile method, over the 72 frames:
the median frame differs by 3.9 levels at the median quantile and 8.3
at the 99th; the 90th percentile frame by 8.0 and 18.4; the worst by
11.0 (a sky frame) and 50.7 (another). That is not the agreement of a
port, and it is not meant to be: they are two transforms with one
ancestry. Where they differ, looked at: Blender's base is lower in
contrast and less saturated than Sobotka's (its matched exposure sits
0.4 stops under Sobotka's Punchy, median +1.88 against +1.46, and its
Punchy is a shadows grade and a small power, not a saturation of 1.4),
and its outset gives saturated colors back more evenly. On the
lanterns the difference is the one that matters: Sobotka's renders
them orange to the core, as §227 saw and preferred; Blender's renders
them a pale salmon with the paper's pink, closer to §226's norm on
that frame than to Sobotka's, since the inset's rotation and the
forty percent hue mix hold the red-orange where Sobotka's per-channel
skew carries it to yellow. On skin and sky, where §230's call was
made, the two agree to a few levels at the quantiles (the greenhouse
portrait 2.9 at the median, the woman in blue in front of a cathedral
1.4) and read the same by eye. So the call carries over on what it
was made on, and the user confirmed it by eye before the port was
redirected: Blender's Punchy beside Sobotka's on the 33 standouts
read "absolutely fabulous overall, maybe even better", the lantern
alley "a bit more salmon but not horrible". The lanterns, which §227
scored for Sobotka, want looking at again with the feel track's look
controls in hand; the frames with the largest differences are the sky set,
blue gradients where Blender's is a touch greyer and lower in
contrast.

**Cost.** On the CPU, single-threaded, release, over four million
pixels of the test colors spread across fourteen stops
(`the_cost_of_a_pixel`, ignored, run by hand): AgX Punchy **136 ns
a pixel**, the norm 65, the curve per channel 4. Sobotka's port was
100 and a first Blender port 181; where the graded color taken back
out of the inset space has no channel under zero, which is nearly
every pixel, the formation's rail and inset would give the inset color
back as it is, so it is handed straight on with its log (one matrix, the
compensation and three `log2` fewer), and `t^1.5` is `t * sqrt(t)`;
both equal to a float's rounding, the oracle and the GPU test
deciding. A 100-megapixel export takes about 0.4 s more over 32
threads. What is left
is the look's pass through the inset and its log, two HSV conversions
and the hue mix. In the shader `tone_agx` is five 3-by-3 matrices, six
`log2`/`exp2`, six `pow` in the sigmoid, three for the decode, three
for the look's power, two `pow(x, 0.08)` where the output rail acts,
two HSV conversions with their branches, and the two rails' min/max
work: about 180 operations a pixel, some 25 of them transcendental. A
4K viewport frame on the RTX 5070 Ti (`the_cost_of_a_4k_frame`,
ignored, release, the parity frame drawn at 3840 by 2160 with the 33
MB read back in the figure): per channel 2.4 ms, the norm 1.7, AgX
1.8; the read back is most of each and the card does not measure the
curve. An integrated GPU and lavapipe remain the viewport's cases to
time before any default changes, as §226 said.

**The tool's AgX columns were 2.2 in sRGB-tagged files.** Found on the
way: before this, the tool wrote every OCIO render as OCIO gave it, and
for Sobotka's AgX views that is the 2.2-encoded value of his config's
sRGB display, in a JPEG a viewer reads as sRGB. A first run of the
port's column put the lanterns at night 8 levels apart at the median,
which was that and not the port. Its size is the two encodings'
difference: up to about eight levels in the deep shadows (a
display-linear 0.003 is 18 levels under 2.2 and 10 under sRGB), under
two from the lower mid-tones up and about a level above the mid-tones
(-1.4 at an encoded 0.5). It touched the judged AgX renders in §227 and
§230, AgX base and Punchy both, and not ACES 2.0 (OCIO's studio config
encodes its sRGB display piecewise) nor per channel or Hold hue
(greycard's own exports); so the AgX shadows judged there were lifted by
up to eight levels, and "slightly flat" has some of that in it, where
skin and sky, in the mid-tones and up, moved by about a level. Blender's
sRGB display encodes piecewise, so the new columns compare as written;
the Sobotka columns are left as OCIO gives them, the docstring says so,
and the Sobotka-against-Blender line re-encodes his for the comparison.