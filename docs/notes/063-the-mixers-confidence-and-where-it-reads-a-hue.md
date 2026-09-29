# 63. The mixer's confidence, and where it reads a hue (2026-09-18)

§61 found the speckle on the dark rock and the patches on the water
were the mixer's: it read a hue off every pixel however little
chroma it had, so on a near-grey surface it read noise, and a band's
luminance lift printed that noise as bright flecks. The fix is the
two parts §61 named.

**Confidence.** `mixer.rs` gains `confidence(chroma)`, a smoothstep
from 0 at grey to 1 at an Oklab chroma of 0.03 (`CHROMA_FULL`), and
`Mixer::at_with(hue, confidence)`, which scales the bands' shift,
chroma change and gain by it before they become the shift, the
scale and the power of two, so at 0 the mixer is the identity
whatever the sliders say. The threshold sits between the rock's
noise as a 5x5 mean sees it (0.004, confidence 0.05) and the water's
real chroma (0.028, confidence 0.98); skin and foliage are 0.08 and
up. Vibrance and saturation are not faded: they scale chroma, which
is already nothing at grey.

**The local mean.** `finish.rs` gains `local_ab`, the Oklab a and b
of every source pixel box-averaged over `MEAN_RADIUS` (2) each way
with the edges clamped, two separable passes under rayon, computed
once an export when any mixer is on. `mix_with` reads the hue and
the confidence from that mean and applies the shift, the scale and
the gain to the pixel's own a and b, so a grey pixel between two
yellow leaves still takes the yellow band's gain and a noisy pixel
on a grey rock does not. The mean is of the source before exposure;
Oklab's a and b are linear in the cube-rooted LMS, so a gain of `g`
scales them by the cube root of `g`, and the finish scales the mean
by that at the pixel's own exposure (vignette and locals included),
exact for a gain uniform over the box. The shader does the same: 25
`textureLoad`s about the source position through the white balance
to Oklab, only when a mixer acts.

**Checked.** The viewport at 1:1 and the export's crop with all
eight bands set differently in a sidecar differ by 0.19 percent
RMSE (0.49 of 255), 0.09 percent mean, against §23's 0.16 for the
mixer before the mean: the shader's taps land on the same pixels
and the difference is the transcendental functions', as before. On
the §61 edit re-exported: the dark rock's luminance residual (L
minus its 2-pixel blur, the grain identical on both sides) fell from
0.97 to 0.80 percent of L, the water's from 1.00 to 0.96, the sky's
unchanged; the green-yellow flecks along the rock's edges are gone
to the eye. A count of dark yellow-green pixels rose, which is the
old picture's lift moving the same pixels out of "dark", and is why
the luminance residual is the measure. The water's olive patches
remain: they are the hue of the water itself drifting across the
aqua and green centers at a scale far larger than the box, which is
the edit, not noise. Tests: the ramp's ends and middle; `at_with` at
0, 0.5 and 1; a near-grey speck in yellow taking no gain from a
grey surround, its ramp's share from itself and the whole stop when
saturated; a grey pixel in a yellow surround taking the stop;
`local_ab` on a flat image, a checkerboard and a clamped corner.
