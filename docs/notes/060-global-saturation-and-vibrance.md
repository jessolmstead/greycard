# 60. Global saturation and vibrance (2026-09-18)

Two sliders, Vibrance and Saturation, in a new COLOR section between
LIGHT and CURVES, acting on the scene-linear picture in the same
Oklab pass as the color mixer (§23). `crates/greycard-edit/src/color.rs`
holds the edit and the one function `finish.rs` and the shader both
hold to, `Color::scale`. Both sliders sit on a track painted from
grey at the left through the mixer's eight swatches, each mixed
with grey by how far along it sits, to full color at the right, so
which way is dull and which is vivid reads without dragging.

**Where and in what.** After the mixer's own hue shift and chroma
scale (`chroma2` in `mix` and `mix_color`), before it goes back to
Oklab's a and b: `chroma3 = chroma2 * Color::scale(chroma2, hue
after the mixer's shift)`. Lightness untouched, as the mixer's own
saturation leaves it.

**The maths.** `sat_scale = max(1 + saturation, 0)`, flat wherever the
pixel sits. `room = clamp(1 - chroma2 / 0.3, 0, 1)`, where 0.3 stands
for "about as saturated as the gamut gets": the Oklab chroma of the
sRGB primaries, red .257, green .295, blue .313. `d` is the shortest
angular distance from the hue to 55 degrees, the mixer's orange
center where skin sits. `protect = 1 - 0.5 * (1 - smoothstep(15, 45,
d))`: vibrance acts at half within 15 degrees of skin, in full 45
degrees away. `vib_scale = max(1 + vibrance * room * protect, 0)`.
The scale is `sat_scale * vib_scale`.

**How locals blend it.** As the mixer's bands do: the global's
color, zeroed when its own switch is off; each local with its
switch on adds its saturation and vibrance in by weight, and turns
the blended color on even if the global was off. The Oklab pass in
`finish_pixel_with` runs when the mixer or the color is on, so a
color-only edit does not pay for an untouched mixer, the same
all-zero-bands-is-identity trick the mixer relies on for itself.

**The Lightroom import.** Vibrance and Saturation used to fold into
the mixer's saturation, Vibrance at half weight, over every band;
they now land on `color.vibrance` and `color.saturation` directly,
the percent over 100, clamped. Black and white
(`ConvertToGrayscale`/Treatment) still greys every band of the
mixer, since that is a hue-band effect, not a global one.

**Checked.** A sidecar with saturation 0.4 and vibrance 0.6 on
`5M0A3976.CR3`: the viewport at 1:1 and the export's crop differ by
0.06 percent RMSE, and differ from a zeroed sidecar's export by 2.9
percent, so the sliders act and the shader agrees with
`Color::scale`.

**Not.** No luminance masking. No separate skin-tone slider: the
55-degree protection is vibrance's own, not a control of its own,
and a picture without skin in it sees the full vibrance wherever
room is short.
