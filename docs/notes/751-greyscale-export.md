# §751. Greyscale export (2026-11-26)

Discussion 207 asked for a greyscale export with a grey profile, and
PR 216 brought it. One space joins the export's: Gray Gamma 2.2. It
writes one channel, an 8-bit JPEG or PNG or a 16-bit TIFF with black
at 0, under a version 2 grey ICC profile. The profile is generated,
and its description says "Gray Gamma 2.2 (compatible)", as our Adobe
RGB's does.

**The render is unchanged.** The picture is finished as an sRGB
export is, with sRGB's matrix, the gamut op (§735), the resize, and
both sharpens. Only the encode differs: each pixel's value is the
luma of its sRGB-encoded channels (Rec. 709's weights), taken to the
grey curve, 563/256, by `Space::recode` and rounded once. On a
neutral pixel that is the pixel; through the embedded profile it is
the sRGB export's light to 3.6e-5 at 16 bits.

**A grey space is a firm choice.** As it came, a frame with any color
in it was written in sRGB instead, per frame and said only in the
log, so that a file never lost its toning. But grain's crystals are
dyed (§33), so a black and white picture with grain, the commonest
monochrome edit, always came out in color, and a batch could mix the
two. Now a grey space always writes one channel. Toning, the dye in
grain, and a colored watermark are taken to their luma. Whoever asks
for a grey file gets one.

**Memory.** The finish encodes each pixel whole as it goes
(`finish_framed` over the channel count), so the grey path never
holds the frame as floats: on a 100 MP 16-bit TIFF its peak over the
frame is 189 MB, where the color path's is 570 MB.

What else:

- **A watermark.** Text is white or black. A PNG is taken to its luma
  on the file's curve.
- **A border.** Every level is neutral. Match is the mean of the one
  channel.
- **WebP.** Its lossless pixels cannot carry one channel under a grey
  profile, so a grey space writes the luma under sRGB's curve there,
  with sRGB's profile and EXIF. The XMP still records the space the
  sheet asked for.
- **Elsewhere.** The viewport shows the picture in color, as for an
  sRGB export. The soft proof does not take the grey space. Edit in
  hands over sRGB in color, since the file comes back through our
  reader.
- **Names.** The sheet's JSON holds the space by name. An unknown
  name, in an older version or a newer one, reads as sRGB.
- **The window.** The grey space is a second row over the same choice,
  on the export sheet and in Settings; with `enters` the right arrow
  reaches it from the color row.

**Not done.**

- Dot Gain 20% came with the PR and was taken out: its curve is
  computed, and nobody has compared it with the profile it is named
  after.
- Our own reader takes a file with a grey profile as sRGB, as before.
  A grey export opened in the editor is off by up to 8 levels in the
  shadows. Reading the curve is small, but it changes how every
  picture with a grey profile already in a library develops.
