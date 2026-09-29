# 61. A Lightroom edit matched by hand: what differed (2026-09-18)

We edited `4Z4A3525.CR3` (R5 Mark II, RF 50mm f/1.2 at f/1.2,
ISO 100) here to match an earlier Lightroom edit, not exactly, and
asked what differed materially and where the artifacts seen at 1:1
came from. Both exports sit in `~/Pictures/Test/export`; the
comparison crops in `export/compare`. The edit: exposure +1.9,
highlights -0.96, whites -0.48, the learned denoiser at "best" and
full blend, sharpen off, a lifted point curve, the mixer's yellow
luminance +0.53 and green +0.23, highlight grading at hue 75 and
saturation 0.16, vignette -1.1, cubic grain at 0.4.

**Method.** The Lightroom export is a 4:5 crop at 5000 tall; this
one a 4:3 crop at native size. Aligned at 1/8 scale, then 600-pixel
crops of each at the same place, the Lightroom one resampled to
match. Then the edit re-exported through the editor with one stage
switched off at a time (`--export`, a sidecar variant beside a link
to the raw): grain, the denoiser, the mixer, the mixer with grading
and color, and from there the curve, the lens, the tone shifts, the
vignette, and finally everything but exposure.

**The speckle is the mixer's.** Green and yellow flecks over every
dark, near-neutral surface (the rock behind the couple, the cliff),
and yellow-green patches across the water where Lightroom's is an
even teal. Present with the grain off and with the denoiser off;
gone with the mixer off. In Oklab on the mixer-free export the dark
rock's median chroma is 0.017 with its hue spread over most of the
circle, the water's 0.028 between 166 and 234 degrees, the foliage's
0.10. The mixer reads a hue from every pixel however little chroma
it has, so on the rock it reads noise and prints it: a 44 percent
gain on whichever pixels fell in yellow. On the water the green
band's lift switches on and off as the hue drifts past the aqua
center. §23 chose no masking; Lightroom's mixer, darktable's color
zones and every other one weight by saturation. The fix has two
parts, and the first alone is not enough since the rock's noise
chroma and the water's real chroma overlap: a soft ramp on chroma
so a near-grey pixel gets little of a band's shift, scale or gain;
and the hue and the weight read from a small local mean of a and b
rather than the pixel, the gain still applied to the pixel. `mix`
in `finish.rs`, the shader's `mix_color`, the tests.

**The texture is the denoiser's.** The shirt's weave and the
lighthouse's surface are smooth here and present in Lightroom's;
with the denoiser off they come back. "Best" at full blend on an
ISO 100 frame takes low-contrast texture with the noise. The blend
should start lower at base ISO, or follow the measured noise, so a
clean frame is left nearly alone by default. Sharpen was off in the
edit, which Lightroom never has by default; some of the softness is
the edit's.

**The grain sits only in the shadows.** In the sky the export with
grain and the one without are the same to the eye; Lightroom's grain
covers the sky evenly. §33 weights the grain toward the shadows, so
it lands on the dark rock, on top of the mixer's speckle, and is
absent where film grain shows most. The tonal weighting wants
revisiting: flatter, or a curve that peaks in the midtones.

**The purple bokeh is the lens's.** The out-of-focus sky gaps in the
trees are lavender here and neutral in Lightroom's. With only
exposure applied, the editor and the CLI agree and the discs are
faintly lavender: the lens's axial chromatic aberration, which the
edit's deep highlight and whites pull-down turns saturated. No one
stage causes it (each of the curve, the lens, the tone shifts and
the vignette off still shows it). §13 said axial CA and purple
fringing are a different tool; it is now on the list.

**Not artifacts.** The beige sky is the highlight grading wheel at
hue 75, saturation 0.16 (a shift of up to 0.2 in a and b at full),
where Lightroom's is blue-grey: the edit, though the wheel is strong
per unit against Lightroom's 0 to 100. Lightroom's export has cyan
fringes at the lighthouse base and along rock edges against the sky
that this one's lens correction leaves clean.
