# §759. A picture's samples, let in finite (2026-11-28)

A NaN or an infinite sample rendered differently on the two paths.
On the CPU the contrast step's `max(0)` turned a NaN into zero, so
the pixel kept a color; on the GPU it drew black. A value past the
largest half float (65504) became infinite at the upload, and
`inf - inf` in the tone shifts made a NaN on the CPU too. In the
window the black spread to the pixels beside it: the shader's texture
filter reading the bad texel, two pixels wide at 1:1 and ten when
magnified.

**One rule, at the doors.** `greycard_core::image::admit`: a NaN is
zero, an infinity and anything past ±65504 is held there, and a
negative is carried, since a color outside the primaries is
legitimate in a linear space. It runs where a picture's samples
enter: the float picture reader, before any resize or blur; the
GPU's upload, before any filter; and the finish's pixel, so the CPU
reads what the texture holds. The shaders test for nothing: WGSL
leaves a NaN's comparisons to the driver.

It is the identity on every finite value in range, so no ordinary
picture moves. One finite case does: a float picture with a sample
past 65504 now finishes on the CPU held at the limit, as the GPU
could only ever hold it.

**A negative channel.** The report that one renders black on the GPU
did not reproduce: 387 colors with a negative channel under fourteen
edits on both curves agree within half a level.

## Whites, and the gamut stage

"Whites down brightens a few pixels" (issue 132's review) is not the
Whites roll. Before the output stage no channel of any color rises
under Whites on either curve, and a test now holds that. What rises
is one channel of a color far outside the output space, after the
gamut stage, as the color darkens: the rail gives `y + (1 - y) *
roll`, and its share grows as luminance falls. Exposure down does it
more than Whites does. On real frames it is rounding, one level of
65,535. We left the gamut stage as it is: any change to the rail
moves every saturated light.

## Left open

- A float raw's odd photosite: the normalize's clamp passes a NaN,
  and it develops as a bright blotch of some twenty pixels.
- A GPU stage's own output is not admitted: a picture at the limit
  pushed past it on the device could go infinite in the viewport's
  half floats. The export reads full floats and admits at the finish.
