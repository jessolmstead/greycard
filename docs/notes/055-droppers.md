# 55. Droppers (2026-09-17)

**What a dropper reads.** The developed picture on the GPU, in the
working space at the white it was developed at: `Renderer::sample`
copies a five by five square of the source texture about the pixel
under the pointer to a buffer, maps it and takes the mean, one
synchronous round trip of a few hundred bytes. The view's point goes
to the source's pixel the way a mask's does, through the geometry.
Off the picture, nothing. Every dropper starts from that sample.

**Neutral, in WHITE BALANCE.** The sample back through the base's
matrix and gains is what the sensor saw; the gains that make that
equal, green at one, are a `WhitePoint::Coefficients`, and rawcolor
resolves them to the illuminant they belong to, whose temperature
and tint go on the sliders, As shot off. Then a develop, since the
gains sit before the demosaic. A candle-lit patch of this church asks
for 1670 K, under the slider's 2000; its stone pillar 2700, near the
3130 the camera chose. The dropper is one shot: a measurement, not a
tool to hold. Raw only, with the section.

**Pick, in CURVES.** The sample at the panel's white, through the
panel's exposure, mixer and tone curve to the encoded value the
point curves read, `finish::pick`, the same stages as
`finish_pixel_with` under the global look alone. The channel decides
which value: the encoded luminance for RGB, the channel's for Red,
Green and Blue, the Oklab lightness after the curves for the color
curves. A point within twice the minimum gap of that x is the one;
otherwise a new point on the curve as it stands, so the press moves
nothing. A drag up or down moves the point's y, 200 logical pixels
for the whole range; the release records the step. The tool stays in
hand for the next tone, Esc or the button puts it down.

**Pick, in COLOR MIXER.** The same stages to the Oklab hue after
the exposure, where the mixer reads it; the band with the greater of
the two weights becomes the panel's. A drag up or down is the band's
saturation, sideways its hue, the same 200 pixels for each whole
range, so a color can be turned and drained without leaving the
picture; luminance stays on its slider. Stays in hand like the
curve's.

**What it is not.** The stages are the global look's; a pick with a
mask's look on the panel reads the picture as the global look leaves
it, which is what the local curve and mixer see, but the vignette's
stops at the pixel are left out. The white balance pick reads the
demosaiced picture, not the mosaic, so a clipped patch reads as the
highlight rebuild left it: pick something grey, not something white.
