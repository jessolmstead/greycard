# 103. Texture and Clarity: local contrast from a guided filter (2026-09-19)

The roadmap's "Texture, Clarity ... sliders in the Detail section but
clearly separate from sharpening". Two sliders, one op, in the
engine at `develop::local_contrast`; the panel's DETAIL section holds
them, and the section that was DETAIL, the capture sharpening, is
SHARPEN now, under it. Dehaze is another agent's and goes beside them.

**What it is.** Each slider is a band of the log luminance (Rec.2020
weights, the working space's, in stops about mid grey), scaled by
the slider and put back as one gain on all three channels, the way
the Light sliders work (§19), so a color keeps its hue and its
channel ratios and the op cannot invent a color cast. Texture is the
fine band: the log less an edge-preserving base of it from a guided
filter at a radius of a two-thousandth of the long edge, never under
two pixels (three on a 24 MP frame, four on 45), grain included.
Clarity is the band between that radius and a fortieth of the long
edge (150 pixels on 24 MP, 205 on 45), about where Lightroom's
feels: the log low-passed at the fine radius, less the guided filter
of that at the coarse one. The radii are fractions of the picture,
not pixel counts, so the worker's full-size develop and any smaller
develop of the same frame would show the same thing. Amounts run -1
to 1, the panel shows ±100; at +1 the fine band is doubled and the
middle one gets one and a half of itself; negative takes the band
away instead, Texture below zero smoothing skin, Clarity below zero
softening the structure while the pores, the lashes and the grain
stay. The two bands do not overlap, so the sliders are independent
and their gains multiply: both at +1 double the fine band, not
quadruple it.

The first cut had Clarity read everything under its radius, the fine
band included, and the review measured what that meant: on the R5
portrait's white door the grey standard deviation went from 0.0083
to 0.0163 at Clarity +1, more than Texture +1 gave it (0.0120), and
the cardigan's knit at Clarity -1 went from 0.032 to 0.006, gone
rather than softened. Banding it is the fix, and the band's lower
bound is a plain low-pass (a box twice at the fine radius, a
triangle) rather than Texture's own guided base, because a guided
filter is an edge-keeper and not a low-pass: at the fine radius, a
window of five, it lets a fifth of a period-six grating and a
twentieth of the grain through into its base, and Clarity would have
lifted them (a first try with the guided base as the bound moved the
test grating by 39 percent). With the triangle a full-size frame's
fine band leaks two percent into Clarity's, and the test holds a
period-six grating within five percent at either end of the slider.
On the portrait after the fix, a patch of the white door (a
different patch from the review's, so the plain numbers differ):
0.0173 plain, 0.0221 at Texture +1, 0.0178 at Clarity +1, 0.0217 at
Clarity -1; the knit: 0.039 plain, 0.055 at Texture +1, 0.065 at
Clarity +1, 0.027 at Clarity -1, softened and still there. The knit
is Clarity's to lift: its period is twenty-odd pixels, inside the
band.

Clarity is weighted to the mid-tones by its own base: full from 5.5
stops under mid grey to 1.5 above, fading to nothing by 8 under and
2.5 above (scene white is 2.47). The stops are the scene's, before
the look's exposure, so the shadow fade is set deep on purpose: a
candle-lit interior at base exposure lives four stops under mid grey
and is still detail the slider should reach; what the fade leaves out
is the noise floor. The first cut faded from 3.5 stops under and did
nearly nothing to the church sample. And both bands fade to nothing
toward the clip, on the pixel's brightest channel from three quarters
of the develop's clip level (four tenths of a stop under it) to the
clip, the level the sharpen masks at: a blown plateau is not detail,
and without the guard Texture +1 lifted the inside edge of one past
the clip and darkened the ring outside it. The worker hands the
base's clip level, the CLI the develop's, a picture that is not a raw
its own.

**Why a guided filter.** The base at each radius is the guided filter
(He, Sun and Tang, ECCV 2010) of its input by itself: in every window
of the radius, the least-squares line `a·I + b` through the window
with `a` shrunk by an epsilon against the window's variance, the two
coefficients averaged over the windows a pixel is in. Where a
window's variance is well above epsilon `a` is near one and the
picture passes through; well below it, `a` is near zero and the
window's mean comes out. So a hard edge comes out where it went in,
and the halo an unsharp mask at a 150-pixel radius would ring it with
(two stops at a four-stop edge, in the test's picture) does not
appear. It is four box filters and a few passes of arithmetic, linear
in the pixels whatever the radius; the box filter is two passes of
running sums, in place (the row pass reads the plane and writes the
scratch, the column pass reads the scratch and writes the plane), the
columns done in bands of rows so it parallelizes, each band summing
its first window whole and sliding from there. The filter works in
three scratch planes and the op holds the log beside them, four
planes of the picture's size in all: 384 MB at 24 MP, 720 at 45,
freed on return. A bilateral at these radii is either a grid
approximation or slow, and a Gaussian base is the halo machine. The
local Laplacian, which is what Lightroom's Clarity is generally taken
to be and what darktable offers beside its bilateral, is halo-free by
construction and the upgrade if the guided filter's own artifact is
felt: a window holding one strong edge protects the texture beside
it too, so detail right against a skyline gets less lift than the
same detail in open ground.

**Epsilon, measured.** In stops squared. Texture's is 0.05 (a fifth
of a stop of standard deviation), small enough that any real edge is
protected and fine grain is what moves. Clarity's was chosen by a
sweep in the test picture, a 0.33-stop bump of six pixels' sigma on
mid grey beside a hard edge, at a radius of 32:

| epsilon | lift on a 0.33-stop bump | on a 1-stop | on a 2-stop | halo at a 4-stop edge |
|---|---|---|---|---|
| 0.5 | 94% | 82% | 57% | 0.39 stops |
| 0.25 | 92% | 71% | 40% | 0.23 |
| 0.125 | 89% | 57% | 26% | 0.13 |
| 0.06 | 83% | 39% | 14% | 0.07 |

The halo grows about in proportion to epsilon and the lift on small
structure hardly depends on it; what a larger epsilon buys is lift on
structures of a stop or two. A two-stop edge's halo is a little
larger than a four-stop edge's at the same epsilon (0.31 against
0.23 at 0.25), since a smaller edge is less protected, and that is
the tradeoff's shape: at some contrast an "edge" is the structure
the slider is for. 0.25 is the middle taken: half a stop of standard
deviation, a third of a stop of halo at a four-stop edge at the
slider's end, a sixth at +50. (The sweep was done on the first cut's
broadband layer; the band's halo at the same edge is what the test
still holds under a third of a stop.)

**The gain.** Texture at +1 doubles its band. Clarity's band lost the
fine energy the first cut let it keep, so its gain went from one to
one and a half to read about as it did, judged on the church and the
portrait. The numbers, grey standard deviation of a crop: the altar
0.086 plain, 0.098 under the first cut at +1, 0.105 under the band at
+1; the face 0.061 plain, 0.091 at Clarity +1, 0.064 at Texture +1,
0.033 at Clarity -1. At +1 the face has the modelling Lightroom's
+100 gives, pores drawn and the shadow under the eyes deepened, and
the grain not raised; at -1 the skin has the glow of a softening
filter with the lashes and the pores kept, which is what the band
buys.

**The sharpen after it.** The sharpen runs on the picture with the
detail in, and its automatic contrast threshold reads that picture.
On the portrait: plain, threshold 11 percent and 27 percent of the
picture sharpened; Texture +1 takes the threshold to 18 and the area
to 25, since the flattest patch is no longer as flat; Clarity +1
leaves the threshold at 11 and sharpens 37 percent, the structure it
lifted crossing the threshold; Clarity -1, 10 and 22. Under the first
cut Clarity -1 took the threshold to 1 percent and sharpened 87
percent of the picture, the flat patches having been flattened to
nothing; the band leaves them their grain and the threshold its
footing.

**Plumbing.** The op takes `LocalContrastOptions { texture, clarity }`
and the clip level on the working image and hands back the radii it
used. The edit has a `Detail { enabled, texture, clarity }` beside
`sharpen`, with serde defaults so an old sidecar reads as on with
both at rest, which asks the engine for nothing; `same_develop`
compares it and `same_patched` does not, so a slider move reuses the
worker's base and the retouch and pays for the op and the sharpen
only. The worker runs it after the retouch and before the sharpen,
on the one copy the sharpen takes, so the sharpen's blend mask reads
the picture with the detail in; the export goes through the same
`develop_job` and sees the same picture. The status line says "local
contrast at 3 and 150 px in 0.11 s" after the develop's time. The CLI
has `--texture` and `--clarity` (-1 to 1, through the edit crate's
`Detail` so a flag past the end is held the way the slider is), run
by `correct_and_sharpen` after the lens and the defringe and before
the sharpen, on the raw and the picture paths both, with a line on
stderr for what it did and how long. The Lightroom mapper takes
`Texture` and `Clarity2012` (or the older `Clarity`) across at ±100
to ±1 into the new section instead of listing them as unmapped;
presets carry a `detail` section, on by default like the other look
sections; the history names a single moved slider ("Clarity +50")
and the sharpen's rows say "Sharpen" rather than "Detail" now. The
panel's fold key "detail" now means the new section, so a settings
file that had DETAIL folded opens with the new DETAIL folded and
SHARPEN open; keys are the panel titles by convention and it is a
one-time shift.

**Tested.** In the engine: the running-sum box mean against a plain
one at five sizes and five radii, edges and bands included; the
guided filter against the paper's per-window least squares on a
20x20 picture at three radii and epsilons to 1e-4, and a four-stop
step with the variance far above epsilon passing through within
1e-3; zero is the identity and a flat field stays flat at any
amounts; a fine grating's spread grows by half or more at Texture +1
and halves at -1; the same grating at Clarity ±1 moves by under five
percent, and both at +1 give what Texture alone gives; a soft
mid-tone bump's spread grows by half at Clarity +1 and halves at -1
while a four-stop edge beside it overshoots by under a third of a
stop on either side; the same bump at 0.9 and at 0.0005 moves under
a fifth of what it does at mid grey; a clipped plateau beside the
grating is untouched at Texture +1, nothing is pushed over the clip,
the grating away from it is lifted as before, and the same picture
with no clip level does go over, so it is the guard that held it;
every channel ratio holds to 1e-4 over a colored picture with both
sliders on; the radii follow the long edge. In the edit crate: the
old-sidecar default, the switch, the clamp, the cache test, the round
trip, the history's words, the preset section count, the Lightroom
sample with `Clarity2012="-35"`.

**Measured.** On the 6000x4000 R6 II church frame, 32 threads: both
scales at once, 0.16 s on the CLI, 0.11 s in the worker; on the
8192x5464 R5 portrait, 0.16 s for Texture alone and 0.21 for
Clarity. The sharpen beside it is ~0.6 s at 24 MP.

**Seen.** CLI previews cropped 1:1 at the altar, plain against the
first cut's Clarity +1 and the band's: the band at one and a half
reads as the first cut did, the gilding lifted and the recesses
deepened, with no ring against the dark wall. The portrait's face at
1:1, plain, Clarity +1, Texture +1 and Clarity -1, as described under
the gain. The editor at 1:1 with Clarity at +50 from a preset laid
over the default edit, the viewport screenshot against the same at
0: a touch more bite in the altar's brass and the lace's edge, 0.88
percent RMSE of full scale between the two (0.57 under the first
cut), and the status line reads "local contrast at 3 and 150 px in
0.13 s". The panel snapshot shows DETAIL with its two sliders over SHARPEN, the
labels within the 68 px column. The export from the editor with the
same preset and with the slider at 0, the same center crop of each:
the export moved between 0 and +50 by about what the viewport moved
(0.66 against 0.57 percent RMSE of full scale under the first cut),
the same op on the same develop; the export's crop against the
viewport's screenshot sits at 1.4 percent either way, at 0 as at
+50, which is §18's half-pixel misalignment of an odd-height
screenshot and not the op.

**Open.** Local Laplacian for Clarity if the guided filter's
protection of texture beside a strong edge is noticed. Clarity -1 on
a flat patch beside a darker edge can put a slow gradient into the
patch (the door's standard deviation rose from 0.017 to 0.022 at -1)
where the 411-pixel windows reach the frame beside it; the
slider's far end, and the same window that softens is the one that
does it.
