# 336. Adobe RGB as an export space, and a curve per space (2026-10-17)

Until now every export space took the sRGB transfer curve: sRGB itself,
Display P3 (which is defined that way) and Rec.2020 (which has its own
BT.1886 and OETF, but which we have always written under sRGB's curve,
with a profile that says so). Adobe RGB (1998) is the first space whose
definition is not that curve: it is a pure power, 563/256 (2.19921875).
A file written under the sRGB curve with Adobe RGB's primaries would
carry a profile claiming a curve the numbers do not follow in the darks,
and every lab and editor that honors the profile would show it
differently from the screen. So the curve became a property of the
space.

## What changed

- `Space::AdobeRgb`, named "Adobe RGB", in the sheet, the setting, the
  presets and the proof's profile keys (the proof takes any space's name).
- The finish still encodes with the sRGB curve everywhere, because its
  curves, its look tables and the grain work on that encoding, and
  changing that would move every picture. At the end, the export's
  quantizer asks the space to `recode` the finished value: unchanged for
  sRGB, Display P3 and Rec.2020, bit for bit; for Adobe RGB, decoded
  from the sRGB curve to light and encoded with 1/gamma. The same
  recode is applied to a watermark picture converted into the space.
- The profile carries the curve: lcms parametric type 1 with the gamma,
  where the other three keep type 4 (sRGB). The monitor section's Adobe
  RGB profile and the export read one constant (`ADOBE_RGB_GAMMA`).
- The viewport: the shader's output is the finish's sRGB-encoded value
  and the display table reads it. For Adobe RGB the table is built from
  the space's primaries under the sRGB curve (`finish_icc`), which is
  the encoding the table is handed, and is the same colors as the file's
  own values under the file's profile. We chose this over a second
  encode in the shader because it leaves the shader, the uniforms and the
  CPU reference of every op alone, and a test holds the two together: on
  an Adobe RGB monitor the table gives the file's recoded values at the
  grid's points (worst 5e-5).

## What we measured

- The profile read back by lcms, a grey ramp from 0 to 1 through the
  recode, against the light it came from: worst luminance error 1.5e-5
  (1/65536; the bar was 1/4096).
- One frame (a Canon CR3, 1600 px long edge, 16-bit TIFF, no sidecar)
  exported to Adobe RGB and to sRGB, both decoded through their embedded
  profiles to XYZ: mean absolute difference 9e-6, worst 4e-5 across the
  1.70 million pixels whose sRGB value is inside the sRGB gamut. The frame
  has few colors outside it; where there are some, the Adobe RGB file
  keeps them and the sRGB one clips them, which is the point of the
  space.
- The three old spaces: `recode` is the identity on all 65536 sixteen-bit
  levels, and a render for each, as JPEG and as TIFF, equals the finish
  quantized the way the export did before.

## Smaller decisions

- The embedded profile's description reads "Adobe RGB (compatible)", as
  darktable's and RawTherapee's do, because the plain name is Adobe's
  mark. The menu entry, the setting and the proof key stay "Adobe RGB".
- The border's fixed greys are levels on the sRGB curve (0.8 and 0.2);
  they are recoded for the file's curve, so a dark grey is the same
  lightness in every space. Match was already right.
- The scopes read the finish's sRGB-curve value, not the file's gamma
  value: the clipping ends are the same, the shadows read differently.
- Adobe RGB is also in the soft proof's control.
