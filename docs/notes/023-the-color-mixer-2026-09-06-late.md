# 23. The color mixer (2026-09-06, late)

Hue, saturation and luminance by hue band, eight bands from red round
to magenta, Lightroom's arrangement. `crates/greycard-edit/src/mixer.rs`
holds the edit; the viewport shader and `finish.rs` apply it; a COLOR
MIXER section on the panel, a row of swatches to pick the band and
three sliders for it, edits it.

**Where and in what.** On the scene-linear picture after exposure and
before the tone shifts and the curve, in Oklab: linear Rec.2020 to
Oklab's LMS (Ottosson's sRGB matrix composed with the working space's
matrix to sRGB), the cube root, his second matrix, then hue and chroma
from a and b. Oklab because its hue is even enough to slice into bands
without a red turning purple as it brightens, and its lightness is
perceptual, at a cost of three cube roots a pixel. Negative channels,
which a wide-gamut scene has, keep their sign through the cube root on
both paths.

**The bands.** Centers in Oklab hue, degrees: red 25, orange 60, yellow
100, green 140, aqua 195, blue 265, purple 300, magenta 335, set from
the Oklab hues of the sRGB primaries and secondaries (red 29, yellow
110, green 142, aqua 195, blue 264, magenta 328) spread a little so
each has room, with orange put where skin sits, which is the band's
reason to exist. A hue lies between two centers and the two bands
share it linearly, so the weights sum to one, only two bands ever act
on a pixel, and a slider's reach ends at its neighbors' centers. A
test walks the circle.

**What the sliders mean.** Hue: a turn in degrees, ±30. Saturation: a
scale on chroma, −1 (grey) to +1 (double). Luminance: a gain on the
band's light in stops, ±1. The last was first a scale on Oklab's L
alone, which at fixed chroma looked washed out as it brightened; a
scale on L, a and b together is a scale on the cube-rooted LMS, which
is a gain in linear light, hue and saturation held exactly, and that
is what a luminance slider should be. The test asks for twice the
light at +1 stop and gets it to three decimals.

**Checked.** With all eight bands set to different values in a
sidecar, the viewport at 1:1 and the export's crop differ by 0.16
percent RMSE (0.4 of 255; the shader's cube root, power and
arctangent are not the CPU's to the last bit) and differ from the run
without the mixer by 6 percent. The parameter block grew by nine
vec4s: the two Oklab matrices and the three sets of bands.

**Not.** No masking of the mixer by luminance (Lightroom's mixer
touches everything of a hue, shadows included; so does this). No
"color" mode with a single saturation and hue per swatch; the sliders
per band are that. Grading by tonal range is the next item and shares
this machinery.
