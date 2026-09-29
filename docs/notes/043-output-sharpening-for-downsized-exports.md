# 43. Output sharpening for downsized exports (2026-09-15)

The second of the editor list, and the note §20 and §21 both left:
a Lanczos downsize softens, and a small picture wants a touch back.

**What it is.** The capture sharpening's deconvolution (§21) run
again on the resized picture, in linear light before the finish, with
a fixed radius in place of the mosaic's measured one, since the point
spread here is the resize's and not the lens's. A Lanczos downsize
by a large factor leaves a spread of about half an output pixel, so
the levels are radii around it: Low 0.45, Standard 0.55, High 0.65,
twenty iterations each, the contrast threshold measured on the small
picture as the capture sharpening measures its own, so its flat parts
keep their grain, and the clip mask from the develop's clip level,
which the worker passes along. Off, Low, Standard and High on the
export sheet, Standard by default, kept in the settings; applied only
when the export is smaller than the picture, and the sheet's label
greys when it is not. The previews the models see (§34) are
rendered with it off: they should see the picture, not a screen's
version of it.

**Measured.** On the R6 II bridge frame exported at 1024 on the long
side, the standard deviation of a Laplacian over the grey picture, a
plain sharpness figure: 0.039 off, 0.048 low, 0.053 standard, 0.054
high; the crops show the branches crisp and no halo. Two things
learned on the way. Iterations do not set the strength: the
deconvolution converges under a small radius, and ten, twenty and
thirty give the same edge, so the levels are radii. And too wide a
radius sharpens less, not more: at 0.8 the halo guard (a tile stops
when any pixel falls under half its blended start) fires early in
tile after tile, and the whole comes out under 0.6. The radius has
to be the resize's; the deconvolution is not an amount knob. A test
sends a soft edge through the three levels and checks they steepen
it in order, leave the flat field alone, and keep the color ratios.

**Beside it.** `--export PATH` exports with the sheet's remembered
choices now, the format from the extension, rather than the defaults
at full size: the choices are the user's preferences for an export,
and the flag is an export.
