# 20. The export sheet (2026-09-06, late)

`Export...` on the panel opens a sheet over the window: format, JPEG
quality, long edge, color space, whether to embed the profile, then
`Choose file...` asks the desktop where. The choices live in the
window for the session; a settings file is on the roadmap (§15).

**Format.** JPEG at a quality (50 to 100, 92 by default), PNG at 8
bits, TIFF at 16. The finish (§18) now takes an output matrix and a
quantizer, so the 16-bit path is the same arithmetic as the 8-bit one
with a different last step, and a TIFF written and read back is
bit-exact in the test.

**Size.** Full, or a long edge of 4096, 2048 or 1024, never enlarged.
The resize is Lanczos on the linear working image before the finish,
where averaging is physically right; the shifts and the curve see the
smaller picture. Lanczos overshoots a little at edges, which the
finish's clamp at zero and the shoulder above absorb. Downsized
exports want a touch of sharpening; roadmap.

**Color.** sRGB, Display P3 or Rec.2020, all with the sRGB transfer
function (P3's own, and a fair choice for 2020 in an 8- or 16-bit
file). Rec.2020 is the working space, so its matrix is the identity
and the export is the working image encoded. The profile is built by
Little CMS from the space's primaries and white with the sRGB curve
as a parametric type 4, named, and embedded in all three formats
(the `image` crate's encoders take an ICC). A test sends a color
through the P3 matrix and the P3 profile to XYZ and through the sRGB
pair, and they agree to two thousandths: the profile says what the
matrix does. Without a profile a wide-gamut file is a lie waiting to
happen, so embedding is on by default.

**Where.** The file chooser is the desktop's, through the
`org.freedesktop.portal.FileChooser` portal over D-Bus with zbus, the
way colord is reached (§17): no GTK, no toolkit dialog, and the same
chooser every other app on the desktop shows, with the RAW's name and
folder and a filter for the format. The portal answers with a
`Response` signal on a request object whose path is predictable, so
the subscription goes up before the call and the answer cannot be
missed; an old portal that names the request itself is handled too.
The wait happens on a thread of its own and the result crosses back
to the event loop, so the window stays alive under the dialog. With
no portal to ask (printed, not fatal) the file goes beside the RAW as
`NAME.greycard.EXT`, as before. `--export PATH` still exports the
first file without asking, the format from the extension.

**What is missing.** Metadata: the export carries no EXIF (roadmap,
§15). The sheet's choices do not persist. And the sheet cannot be
driven from the command line for a screenshot, so its look is our
call, like the rest of the panel.
