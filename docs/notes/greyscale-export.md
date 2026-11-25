# Greyscale export (2026-11-25)

Discussion 207 asked for a greyscale export with a grey profile. Two
spaces join the export's: Gray Gamma 2.2 and Dot Gain 20%, the standard
profiles' names. Each writes one channel: an 8-bit JPEG or PNG, or a
16-bit TIFF with black at 0, under a version 2 grey ICC profile.

**The render is unchanged.** The picture is finished as an sRGB export
is, with three channels, sRGB's matrix, the gamut op (§735), the resize,
and both sharpens. Only the encode differs. The finish hands its encoded
floats over unquantized, and `Space::recode` takes them to the grey
space's curve as it takes Adobe RGB's, so a value is rounded once.

- Gray Gamma 2.2 is 563/256, what a profile's single gamma entry holds.
- Dot Gain 20% prints an ink dot of `a` as `a + 0.8 a (1 - a)`, 70% at
  half. Its profile is a printer's with a 1024-entry table.

The value is the encoded channels' weighted mean. On a neutral pixel that
is the luminance: through the embedded profile it is the sRGB export's to
3.6e-5 at 16 bits, and to half a level of each file at 8.

**A picture in color is written in sRGB.** The rule is that a file never
loses toning. It is decided on the finished pixels and not on the edit:
a look table, a camera profile, a mask, or the photo itself may put color
in, and the edit does not say whether one does. The cost is one pass over
the frame's floats, which the grey export holds whole (12 bytes a pixel
until the file's samples are made). If any pixel's channels are more than
1/4096 apart, encoded, the same floats are quantized as sRGB's, so the
file is the sRGB export's byte for byte, with sRGB's profile and name in
the XMP. The log says so; the window does not.

1/4096 is sixteen 16-bit steps and a sixteenth of an 8-bit one. Ten edits
of three equal channels (both display curves, pushes, the color sliders,
black and white, the vignette, the output sharpen, and a mask) came out
at most one 16-bit step apart. The least of the Tint and Grading sliders,
0.01, is past it.

What else counts:

- **Grain.** Its crystals are dyed (§33), so a picture with grain is in
  color and is written in sRGB. Grain without the dye on a monochrome
  picture would be a change of look, and is not made here.
- **A watermark.** Text is white or black. A PNG with any visible color
  makes the file sRGB's. A neutral one is laid on in the grey curve.
- **A border.** Every level is neutral and stands where it does in the
  other spaces. Match is the mean of the one channel.
- **WebP.** Its lossless pixels are always ARGB, where a grey profile
  means nothing. A grey space writes sRGB there.

**What a grey space does elsewhere.** The viewport shows the picture as
for an sRGB export, and the gamut op is sRGB's there too. The soft proof
does not take the grey spaces. Edit in hands over sRGB, since the
file comes back through our reader.

**Names.** The sheet's JSON holds the space by name, in the settings, the
presets, and the queue. An older version reads an unknown one as sRGB, as
it would any new space. `Space::ALL` stays the color spaces, and
`Space::GREY` holds these.

**The window.** The color row has no room for two more names, so they are
a second row over the same choice, on the export sheet and in Settings.
A `Segmented` that does not hold the current option ignored the arrows;
with `enters` the right arrow takes its first, so the keys reach both
rows.

**Not done.** Our own reader (`MatrixTrc::parse`) takes a file with a grey
profile as sRGB, as it did before. A grey export opened in the editor, or
its thumbnail, is off by up to 8 levels in the shadows under Gray Gamma
2.2 and up to 25 under Dot Gain 20%. Reading the curve is small, but it
changes how every picture with a grey profile already in a library
develops, so it is left for a decision.
