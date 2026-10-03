# 230. AgX over ACES 2.0: the call from two sets, frame by frame (2026-10-03)

§227 left two finalists and the call waiting on the whole test set and
a shoot or two, judged by eye with skin watched. We ran it today, on
two sets: every raw in the test folder, 72 frames once the two Nikon
High Efficiency NEFs are left out (rawler does not decode them), and
72 frames from the archive, two drawn at random from each of its 38
shoot folders so no shoot dominates and the bodies fall out in
proportion (Canon R6 II, R5 II, R8, R6 III, Fujifilm GFX 100S II). Each
frame five ways with `tools/compare-transforms.py`: per channel
(today's default), ACES 2.0, AgX Punchy, AgX base and Hold hue to
white, at matched median luminance, laid out one row a frame by
`tools/transform-sheets.py` and judged by scrolling. The first set got
a line a frame; the second, standouts only, since the pattern was set
by then. The judge's notes stay out of the repo; this section is what
they add up to.

**The first set, 72 frames, about 60 with a verdict.**

| verdict | frames |
| --- | --- |
| ACES 2.0 or AgX Punchy as the base | about 38 |
| per channel, as the finished one | about 10 |
| AgX base best (very high contrast or very blue scenes) | 4 |
| Hold hue to white best (a neon sign per channel clipped) | 1 |
| Hold hue to white harsh or too bright | about 40 |
| AgX Punchy explicitly over ACES 2.0 | 8 |
| ACES 2.0 explicitly over AgX Punchy | 4 |

Hold hue to white is settled: whites too bright, skin hotspots hot,
lanterns salmon, on forty frames. It won once, where per channel
clipped a neon sign's chroma. The step is the fault, as §226 and §227
said, and the step does not need fixing if the transform changes.

Per channel wins the low-contrast and overexposed scenes, where it
reads as finished and the others as flat, and loses every scene with
range: shadows go dark on contrasty scenes, snow reads like an HDR
display, blues, greens and neon over-saturate. Those are the three
places the per-channel curve's slope spread shows (§226).

ACES 2.0 and AgX Punchy are judged as a base, "a great starting point
to edit", with "slightly flat" beside it almost every time. Part of
that is contrast rather than the transform: the exposure match
equalizes brightness, not contrast, and per channel's finished look
is partly §145's shoulder. ACES 2.0's faults are consistent and of one
kind: a greyer, less saturated blue sky on six frames, skin washed out
on two portraits ("like a ghost"), detail lost on the moon, and a
slight shift in the saturated colors on three Fuji frames. AgX
Punchy keeps the sky, keeps skin and takes the lanterns; its faults
are a red coat too saturated and pushed orange, and lanterns very
bright and saturated.

**The archive set, sixteen standouts.** Skin decided it. Punchy was the
favorite on four portraits and "sooo good" on two; ACES 2.0 a close
second twice and the winner once; ACES 2.0 washed skin out on four
frames, the same fault again. Per channel won the overexposed and flat
frames and was "what I expect" on reds, and lost on skin hotspots and
on contrasty, saturated scenes. On the biggest difference of the day,
a sunlit tower with a subject in its light, per channel made the tower
and the light on the subject yellow and saturated, Hold hue was
better, and ACES 2.0 and Punchy both gave the subject natural color,
ACES 2.0 slightly the better skin and Punchy the better tower. Reds go
pink on both ACES 2.0 and AgX, a red locker and a red shirt: both hold
hue where per channel skews a red toward yellow, and the eye expects
the skew.

**The shift that is not a cast.** The shift seen on ACES 2.0 and AgX
base was measured on five of the frames it was seen on: the pixels
near neutral in the per-channel render (Oklab chroma under 0.015,
lightness 0.25 to 0.85), their mean a and b in each render. All three
OpenColorIO renders move together, by two to fifteen thousandths,
Punchy among them, so there is no cast peculiar to the two. What is
seen is in the saturated colors, and part of it is the comparison's
own: the OpenColorIO renders start from the CLI's linear develop and
per channel from the editor's export, which are not one pipeline
(§227). The next round renders per channel from the same linear file.

**The call.** AgX, with Punchy's look as the default, is the display
transform to port; ACES 2.0 is left, with its numbers in §227. Over
both sets Punchy won on skin and sky, where ACES 2.0's faults were
consistent and go the wrong way for a look to fix: a greyer sky,
washed skin, pink reds and the moon are chroma and gamut decisions
inside its appearance model, and a look over its output cannot put
back what the model took. Punchy's faults are over-saturation, which
a look control pulls back. §227 read the gates and the lanterns as
correctness and preferred ACES 2.0 on them; two sets of real frames,
skin across the range, read the other way, and the look is what the
editor's user sees.

The port is cheap, where ACES 2.0 was a module at 243 ns a pixel. AgX
as Sobotka's config has it: an inset of the primaries by a 3 by 3
matrix, a log encoding over about sixteen and a half stops (lg2 from
-12.47 to 4.03), one 1D contrast curve, and the Punchy look a CDL on
the result, power 1.35 and saturation 1.4. A few lines of math and a
few nanoseconds; the shader's cost is a matrix and a curve. darktable
ships an AgX module under the GPL with the curve's shape and the
inset as parameters, which is the source our license allows, and its
parameters are the controls the feel track will want. It goes behind
the existing display-curve switch beside per channel, which stays as
the fallback: it wins the flat and overexposed frames, and a tester
can compare. Hold hue to white as built goes with the old curve; the
question it asked, a hue held through the roll-off, is what the inset
primaries answer.

**The camera match depends on it.** The match fits its matrix, curves
and 33-cubed table on display-referred sRGB after the display curve
(§180), so every table fitted so far encodes the per-channel curve's
behavior and corrects it: the yellow skew of reds, the slope's chroma,
the hard whites. Applied after AgX the same table corrects faults no
longer there and misses AgX's own, and a table under the wrong
transform is worse than none. So a fitted table records the display
transform it was fitted under, the finish applies it only to a
picture on that transform, and on a mismatch the Look section says
"fitted under per channel" and offers the refit; today's tables carry
no tag and are read as per channel, which they are. The fit action
develops through the editor's own pipeline, so it fits under whatever
transform the run is set to, with no change to the fitter beyond the
tag. The upside is that the table then has less to do, since the base
already keeps skin, skies and highlights near the camera's, so it
should generalize better than §180's thin-frame GFX fit, and the pink
reds are exactly its job: the camera JPEG skews them the way the eye
expects, and the fitted table pulls AgX's toward that per body rather
than our choosing a global skew. The fitted error against §181's
per-channel fits is the number that confirms the call.

**The tool.** `compare-transforms.py` now runs the release binaries
(the debug ones took four and a half minutes a frame against forty
seconds), skips a raw already rendered so a run resumes, and prints
and skips a render that fails or passes ten minutes rather than
stopping the set; `transform-sheets.py` lays the renders out.
Both runs went under a headless weston (`--backend=headless
--renderer=gl`, on its own socket) once the editor's windows, two a
frame, made the judging impossible on the same screen; the GL renderer
matters, since the pixman default loses the NVIDIA surface and the
export falls to llvmpipe. An export under it is pixel-identical to one
on the desktop. A headless `--export` of the editor's own is on the
roadmap. On the desktop, two of about ninety hold-hue exports stalled
for minutes at low CPU; forty under weston did not, so the stall is
the window's and not the curve's, and it is on the roadmap as a bug.
