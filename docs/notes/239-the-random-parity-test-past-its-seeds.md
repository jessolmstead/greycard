# 239. The random parity test past its seeds: a NaN in AgX, the color shifts at black, unsettled answers and lit pixels (2026-10-04)

The roadmap's bug: §221 set the parity test's tolerance and its steep
cap from seeds 1 to 10, and past them seed 23 edit 69 failed the 15
percent cap on NVIDIA. Built by an opus author and read twice by an
opus reviewer, who looked at the largest change to a real render
before it landed. We ran the seeds further out, on every driver
the desktop has: NVIDIA (the RTX 5070 Ti, the default adapter),
lavapipe, and RADV, which is there after all: the Ryzen's integrated
Granite Ridge GPU, reached with `VK_DRIVER_FILES` set to
`radeon_icd.json`. Seeds 1 to 100 on all three: 40 000 edits per
driver. Fourteen edits failed on at least one driver (thirteen more
than the one the roadmap knew of), and they fall into four kinds.
Two are bugs, one in the CPU's arithmetic and one in the shade's
definition at black, and we fixed them on both sides. Two are the
test's bounds, and we put each on a measured footing instead of a
number set from ten seeds.

**The sweep, before.** In levels from the nearest CPU answer, or the
steep share for the cap (the steep share is the CPU's own and the same
on every driver). The parts are the ones whose removal brings the edit
within the bounds.

| seed, edit | NVIDIA | lavapipe | RADV | kind | the edit, reduced |
| --- | --- | --- | --- | --- | --- |
| 13, 185 | 2.7 | 3.5 | 2.6 | shift at black | point curves take a grey to exactly black; a color curve's end at black off neutral; AgX |
| 16, 43 | pass | 4.1 | pass | shift at black | point and color curves, black and white, a local |
| 23, 69 | 15.5% | 15.5% | 15.5% | steep cap | exposure +5, highlights +2, a picture already rendered (a clip), the master curve 0 to 0.96 over the first hundredth |
| 35, 260 | NaN | NaN | NaN | AgX overflow | a highlight under stacked light and a local, AgX |
| 41, 1 | 19.8% | 19.8% | 19.8% | steep cap | light, point curves, a rendered picture |
| 50, 60 | pass | pass | 3.8 (12.4%) | unsettled | light, point curves, color, a rendered picture; a lone green primary |
| 50, 292 | 6.8 | pass | 6.8 | shift at black | a local's grading, shadows wheel at 0.51; point curves to 1e-9; a look table |
| 55, 121 | pass | 25.4 | 12.0 | unsettled | exposure +5, contrast summed to 0.5, a local's tint at full; green out of the Oklab pass at 0.0029 against red 5.7; a point curve rising at zero |
| 56, 295 | NaN | NaN | NaN | AgX overflow | exposure +5.8, the vignette's +5 and a local's +4.5, whites summed to 3.7, contrast 2.6: a channel at 9e32 into AgX |
| 64, 385 | 20.1% | 20.1% | 20.1% | steep cap | light, point curves, a rendered picture, the output space |
| 65, 166 | 4.5 | pass | 3.5 | unsettled | a near grey under mixer, color, tint, the color curves and a local |
| 74, 395 | 21.4% | 21.4% | 21.4% | steep cap | light and point curves |
| 81, 64 | 15.7% | 15.7% | 15.7% | steep cap | light, point curves, a rendered picture |
| 84, 341 | pass | 2.7 | pass | unsettled | a lone blue primary under grading, color, tint and the color curves |
| 98, 220 | pass | pass | 3.8 | shift at black | exposure -4, whites +1.4, a local at weight 0.99999994 over point curves that end at black, the color curves |

Seeds 12, 16, 29 and 32, which the roadmap named as failing under the
earlier draws, pass on all three drivers. Seed 16 fails edit 43 on
lavapipe instead, and that edit is a shift at black. A first pass
over seeds 101 to 150 on NVIDIA and lavapipe, with the first fixes
in, found two more: seed 129 edit 102 on lavapipe (unsettled, below)
and seed 138 edit 260 on both drivers (a shift at black whose residue,
2.7e-7, was over the first black point we chose).

**A NaN in AgX.** Seed 56 edit 295: the light stacked under two masks
and the vignette's corner gives `shape` a channel at 9e32, finite.
AgX's Punchy look is a power of 1.09 on a 25-stop log, which stretches
a log2 of 111 to 130, and `exp2` of that is past f32: an infinity, and
the HSV conversion after it makes inf minus inf, a NaN. The CPU
answered NaN, which the export writes as black. The shader went
through the same infinities and wrote whatever its driver made of
them. The overflow starts at a channel near 2^108 before the gain.
Every color, of any kind and both constructions, is exactly white from
a largest channel of 2^6 on: the inset mixes at least a twentieth of
the largest channel into every channel, and the curve is past white
for all three. So `Agx::apply` and the shader's `tone_agx` return
white for a channel at or past 2^64 (`WHITE_PAST`), where they already
returned white for anything not finite. No answer changes.
`past_the_top_every_color_is_white` sweeps the primaries, the
secondaries and 49 mixtures at 2^6 to 2^63 through the arithmetic, and
from 2^64 to `f32::MAX` through the shortcut. The shader's literal is
held to the CPU's with the others.

**The color shifts at black.** Four edits on the flat side of the
bracket: every CPU answer within a tenth of a level of the others, and
the GPU 2.7 to 6.8 levels away. In each one the point curves take the
pixel to black, and a shift of a and b at lightness zero gives black a
color. The shift comes from a color curve's end at black dragged off
neutral, or from a grading's shadows wheel, which has all of black by
design. The color the shift gives is reconstructed through Oklab's
cube, from a lightness that is the cube root of the pixel's light, and
the cube root has no bound on its slope at zero. What comes out of the
point curves at black is not zero on both sides. It is a residue: a
mask's weight one rounding short of one (0.99999994) leaves 6e-8 of
the encoded value before the master curve, a fused multiply-add leaves
its own, and the two sides leave different ones. Through the cube
root, 1e-9 of light is a lightness of 1e-3, and 2.7e-7 (seed 138,
after a master curve's slope) is 6.5e-3. Under a strong shift, through
a dark channel of the output matrix and a look table, that is whole
levels.

This is the roadmap's color-curve-near-black item, and wider than the
item says. The test kept a color curve's interior points over 0.05
for it, but the end at black and the grading's shadows wheel are the
same mechanism, and the test drew both.

The fix, in `shade_by` and the shader's `shade`, gives the shift a
black point. Under `SHADE_BLACK`, 2e-6 of linear light in the largest
channel, the shift reads the pixel as black. From `SHADE_LIGHT`, 4e-6,
it reads the pixel as it is, with exactly the old arithmetic. Between
the two it reads the light scaled by the cube of `g = t (2 - t)`, where
`t` is the way across. The lightness it reads, a cube root, is then the
pixel's own lightness scaled by `g`. That rises from black with a
bounded slope and meets the pixel's own lightness at `SHADE_LIGHT`
with no corner.

We tried two fades first. A smoothstep's square at its foot gave the
cube root back an unbounded slope, and the test's step check found it.
The cube of `t` alone met the old arithmetic three times as steep, the
4.3x kink the review measured. Under `SHADE_LIGHT` the pixel keeps its
own light and takes only the shift's change, as the faded light reads
it: `c - read + shade(read)`. A pixel with no shift is untouched, and
exact black gets the color it always got.

The black point follows from the residue's bound. A cell of the master
curve's table is 1/255 wide and holds at most the whole range, so the
table rises at most 255 times as fast as its input. A rounding of the
encoded value (6e-8) is then at most 1.5e-5 encoded, 1.2e-6 linear,
and `SHADE_BLACK` sits over that. The window is as short as the parity
test passes with. On real exports (below), 2e-6 to 4e-6 changes about
40 percent fewer pixels than 2e-6 to 2e-5, and a third as many as the
first version's 1e-5 to 1e-4. Tighter still, 1.5e-6 to 3e-6, saves
another 8 percent, but it leaves only a quarter's margin over the
bound, so we did not take it. We first tried 1e-7 to 1e-6, and seed
138's 2.7e-7 was over that black point.

**Renders change in near-black under a strong shift at black.** The
export is not the same as before there, and that is by design. Under a
shift at black, a pixel with light between nothing and a few millionths
got a cast set by that sub-level light through the cube root: the CPU
export alone was unstable there, and a mask weight of 0.99999994
instead of one moved it by whole levels. Those pixels now read as
black, so they take black's cast. We measured with a release build that
had a switch back to the old shade, on four raws (a Canon CR3, a
Fujifilm RAF of 100 megapixels, a Nikon NEF and a Panasonic RW2), at
full size, as 8-bit PNG, against the old shade:

| edit | window | pixels changed | changed by 4 levels or more | most | mean over changed |
| --- | --- | --- | --- | --- | --- |
| a full shadows wheel, no crush | any | none | none | 0 | 0 |
| master curve crushing everything under 0.08 encoded, full shadows wheel (hue 220) | 1e-5 to 1e-4, cube (first version) | 1.2 to 5.6% | 0.002 to 1.3% | 5 | 1.4 to 2.3 |
| same | 2e-6 to 2e-5 | 0.36 to 2.3% | none | 3 | 1.0 to 1.4 |
| same | 2e-6 to 4e-6 (shipped) | 0.20 to 1.2% | none | 3 | 1.0 to 1.4 |
| same crush, blue-yellow color curve 0 at black and neutral from the panel's 0.01 | 1e-5 to 1e-4, cube | 0.79 to 3.9% | 0.74 to 3.6% | 48 | 34 to 38 |
| same | 2e-6 to 2e-5 | 0.36 to 2.0% | 0.34 to 1.9% | 48 | 30 to 35 |
| same | 2e-6 to 4e-6 (shipped) | 0.22 to 1.3% | 0.22 to 1.2% | 48 | 35 to 41 |

The ranges run over the four frames, and the 100-megapixel RAF is the
top of each. The changed pixels are the toe of the crush: the
Fritsch-Carlson curve leaves 0.08 with zero slope, so a band of input
just over it comes out as a few millionths of light. The shadows wheel
moves them by at most 3 levels. The knee curve is the worst case
there is, with a whole cast that changes within a lightness of 0.01,
under the black point's 0.0126. There the band's pixels go from the
old arithmetic's partial cast to black's full cast, 48 levels: the
cast region's edge moves out by that band. The old edge sat where a few
billionths of light put it, and any rounding moved it.

`the_color_shifts_are_still_at_black` checks five things:
- residues from 1e-9 to 1.9e-6 take black's color plus their own light;
- the unfaded arithmetic moved such a residue (0.8 of a level on that
  test's curve alone);
- nothing changes from `SHADE_LIGHT` up;
- no step through the fade exceeds a tenth of a level, and a curve
  level at black leaves the light alone;
- the shader's two literals match the CPU's.

The knee curve's whole transition now sits under the black point, so
the test's 0.05 floor on a color curve's interior points may no
longer be needed. Lifting it to the panel's 0.01 is the rest of the
roadmap item, and we have not tried it here.

**Unsettled answers.** Four edits, each on one driver or two, with the
GPU between two CPU answers and within the tolerance of none. Seed 55
edit 121 shows it plainly. Green leaves the Oklab pass at 0.0029
against red's 5.7, a contrast under one takes its power, and a point
curve rising at zero lifts it. The answers run from 57 to 242 levels,
and even the finest nudge, 3e-7 of the largest channel (two to five of
its roundings), moves the answer 160 levels. At f32's precision the
CPU's own answer is not settled there, and the GPU (lavapipe at 134,
RADV at 171) lands somewhere on that continuum. So where the finest
nudges alone spread a channel's answers wider than the tolerance, the
GPU is held to the range of all the answers, not to the nearest one
(`FINEST`). The other three (seed 65 edit 166, seed 84 edit 341 and
seed 50 edit 60) are the same: their finest nudges spread 8 to 32
levels. A black still fails where the range does not reach it, and a
pixel whose finest answers agree is still held to the nearest one.

Seed 129 edit 102 on lavapipe is the case the finest nudges miss.
Green leaves the Oklab pass at 0.055 from nothing, and the blacks'
crush at 0.054 leaves 9e-4 of it for a steep curve to lift. The
finest nudges agree, and the coarser ones spread from 186 to 255
levels with lavapipe 7.9 levels from the nearest. The answer there is
continuous and steep at the scale of the nudges, and 25 samples leave
gaps. So for an edit where a channel of the GPU lies between two
answers and within the tolerance of none, the CPU runs again over a
finer sampling of the same neighborhood. That is 64 steps evenly in
the log from `NUDGE` down to the finest, each channel alone and all
three together, both signs (`REFINED`, about 0.8 s for an edit that
needs it; once in 60 000 edits on lavapipe). Lavapipe then
comes within 2.3 levels of an answer. A value off every answer of the
finer sampling still fails, so an answer that jumps (the tint's tie
at 180 degrees, a branch) cannot pass between its sides.

**Lit pixels and the steep cap.** The five edits over the cap (seed 23
edit 69 at 15.5 percent up to seed 74 edit 395 at 21.4) are the CPU's
own spread. Mapping their steep pixels showed every one of them has an
empty channel: a channel at or under the nudge, where the nudge, a
share of the pixel's largest, is not a small change of that channel
but light where there was none. The frame has 461 such pixels of its
1536: the four rows of lone primaries (12.5 percent alone), the Oklab
field's colors clipped at a channel, and the near-black rows' empty
channels. An edit with a curve steep at zero makes every one of them
steep, which is what the frame puts them there to probe. The draws
reach such curves often: a point at the panel's 0.01 rising, under the
encoding's 12.92. None of the five had a steep lit pixel.

So the cap now counts only the lit pixels, those where every channel
holds more than the nudge puts in. Over seeds 1 to 150 the steepest
lit share is 0.78 percent (seed 63, edit 133), so the cap is 3 percent.
A break that made answers jump would show in many times that. The
empty pixels are still checked against the answers, and they still
count in `STEEP_MEAN`, the run's mean, which peaks at 0.15 percent
against its 1 percent.

**Unsettled answers, capped.** Holding a channel to its range is a
looser check than holding it to the nearest answer. Nearly every
unsettled channel is in the frame's empty pixels, which the lit cap
does not count, so without a bound of their own a change that made
those answers chaotic in a few edits in a hundred would pass. Over
seeds 1 to 150 the most unsettled channels in one edit is 630 of the
frame's 4608 (seed 133 edit 81, the 25.2 percent steep edit, every
steep pixel of it empty), then 425 (seed 74 edit 395) and 223. The
most in a 400-edit run is 631, 1.6 an edit. So an edit fails over 1000
unsettled channels (`UNSETTLED_MAX`) and a run over 5 an edit on
average (`UNSETTLED_MEAN`), a half and three times over the measured
worst. A few percent of edits gone chaotic, at 300 channels each, is
12 an edit and fails the run. The summary line gives each run's most
and which edit. `FINEST` is now written as `2 * 3`, the last step's
two signs over three channels. The test asserts that `NUDGES` runs
from the largest step down and that the answers number `NUDGES.len() *
FINEST`, so a change to the nudges' layout cannot quietly point it at
the wrong answers.

**After.** With all the changes in and the final window (2e-6 to
4e-6), seeds 1 to 150 on NVIDIA and lavapipe, and seeds 1 to 10 plus
the ones that failed there (13, 50, 98) on RADV, all pass:

| driver | seeds | failing | most levels from the answers | steepest edit, all pixels | steepest edit, lit pixels | most unsettled in an edit | refined |
| --- | --- | --- | --- | --- | --- | --- | --- |
| NVIDIA | 1 to 150 | 0 | 1.13 | 25.2% (seed 133, edit 81) | 0.78% (seed 63, edit 133) | 630 | 0 |
| lavapipe | 1 to 150 | 0 | 1.11 | 25.2% | 0.78% | 630 | 1 (seed 129, edit 102) |
| RADV | 1 to 10, 13, 50, 98 | 0 | 1.22 | 12.4% | 0.49% | 56 | 0 |

The wider window, 2e-6 to 2e-5, passed the same seeds where we ran
them (the targets and 1 to 10 on all three), and an earlier pass of
the first version passed RADV through seed 112. All fourteen edits
from before, and the two from the second pass, pass on every driver
they failed on. The worst distance, about a level, is where §231 left
it for seeds 1 to 10 (1.28 on NVIDIA).

**Cost, and running the sweep.** The default run, seed 0x67726579 and
400 edits, in debug, niced, with `RAYON_NUM_THREADS` and
`LP_NUM_THREADS` at 4 on an otherwise quiet desktop (load about 8):
17.4 and 17.7 s on NVIDIA before the change, 17.3 and 17.1 s after;
17.4 s on lavapipe before and 17.5 and 17.3 s after. The finer nudges
cost nothing when no edit needs them, and about 0.8 s for one that
does. A seed is about 17 s, so a hundred is about half an hour per
driver. That is not a default, and the default stays one seed. To
sweep, in fish:

for s in (seq 1 100) set -x GREYCARD_PARITY_SEED $s cargo test -p
greycard-ui -- render::tests::the_shader_is_the_cpu_over_random_edits
--exact or echo "seed $s failed" end

and for lavapipe first `set -x VK_DRIVER_FILES
/usr/share/vulkan/icd.d/lvp_icd.json; set -x WGPU_BACKEND vulkan; set
-x GREYCARD_REQUIRE_GPU 1` (RADV: `radeon_icd.json`). Run at most two
at a time, niced, with `RAYON_NUM_THREADS` and `LP_NUM_THREADS` at
4: twelve at once took the desktop's load past 500. The summary line
now gives the lit pixels' steepest share and how many channels were
held to their range or refined. A failure prints the steep share of
the lit pixels alongside the whole.
