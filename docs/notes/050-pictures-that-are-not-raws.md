# 50. Pictures that are not raws (2026-09-17)

JPEG, PNG and TIFF open in the editor and the CLI. A picture is
somebody's rendering already, white balanced, matrixed, tone curved
and encoded, and the engine does not pretend otherwise: it takes the
samples through the file's own curve and primaries into the working
space, turns them upright by the EXIF orientation, and hands them on
where a raw's develop ends. `greycard_core::picture` does this
(`decode_picture_path`, a `Picture` with the image, the make, model,
shot and ISO from the EXIF, rawler's `RawMetadata` for the export to
carry, and a word on what the samples were taken to be); the
`Decoder` trait stays raw-shaped, since a picture has no sensor to
describe.

**Color.** An embedded ICC profile is read when it is a matrix/TRC
one (sRGB, Display P3, Adobe RGB, Rec.2020, ProPhoto: the colorants
relative to D50 and a curve per channel, `curv` tables and gammas or
`para` parametric curves), the colorants adapted from D50 to the
working white by Bradford and into Rec.2020 by rawcolor's matrices;
its description names the space in the panel. A profile of lookup
tables is not read, and a file without one is taken as sRGB; either
way the picture says so. Sixteen-bit samples go through a
sixty-four-thousand-entry table of the curve, eight-bit through a
short one; floats are taken as linear. Checked on the engine's own
exports: the R6 II frame exported as sRGB, Display P3 and Rec.2020
PNGs differs between the three by 1% RMSE as encoded and by 0.08%
once each is opened through its profile, the remainder eight-bit
quantization and the sRGB gamut's clipping. A hand-built profile in
the tests lands on rawcolor's sRGB matrix to 0.002 and a P3 red
comes out redder than an sRGB red.

**Metadata.** The EXIF is a TIFF structure in every container, so
rawler's reader parses it as it does a raw's: a JPEG's APP1 payload
(the image crate hands it over without its `Exif\0\0` header), a
PNG's `eXIf` chunk, a TIFF's own directories. Make, model, lens,
focal length, ISO and orientation come out of it; the export carries
the lot, so a JPEG re-exported still says which lens took it. The
lens database looks a picture up by make, model and shot
(`lookup_shot`, `profile_for`) as it does a raw, so a JPEG straight
from the camera gets its profile's distortion, aberration and
vignetting corrected as the raw would have (the same sensor size
assumed, which holds for a camera's own JPEG and not for a crop).

**The editor.** The worker's input is a raw or a picture; a picture
is its own base, with no gains and no matrix for the white balance
preview, no measured sharpen radius (the sharpen's default stands),
and the clip level a raw's fraction of white. WHITE BALANCE, NOISE
and DEMOSAIC say "raw only" in their headers and their controls are
greyed; DEMOSAIC's body says what the picture was taken to be. A
picture's fresh edit starts with the capture sharpening off, since
the file has been sharpened once by whatever rendered it
(`Edit::for_picture`); a sidecar is a sidecar, `IMG.jpg.gcd`. The
filmstrip shows the picture itself, downscaled and turned. The CLI's
`info` says the depth, the space, the camera and the lens;
`develop` applies the lens flags and writes the TIFF or the preview,
and refuses a DNG, there being no camera space to write. The lens
correction and everything after it are as for a raw, and the
learned denoiser, which wants a mosaic, is never asked.
