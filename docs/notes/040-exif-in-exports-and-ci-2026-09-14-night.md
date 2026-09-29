# 40. EXIF in exports, and CI (2026-09-14, night)

Two of the Next up. The export had carried nothing about the exposure
since §20; a picture of the moon said neither the lens nor the shutter.

**What an export says.** An export is a new picture of an old
exposure. What the camera said about the exposure is still true of
it and travels: the lens, its serial, the shutter, aperture, ISO, the
metering, the time it was taken with its zone, the GPS block. What
describes the file is the export's own and is set fresh: the size in
`PixelX/YDimension`, `Orientation` 1 (the picture is written the way
up it is shown; the DNG of §12 is the exception, in sensor orientation
with the tag saying so), `Software` "greycard VERSION", `ModifyDate`
the export's time in UTC with `OffsetTime` +00:00 beside it (the
original's zone stays on `OffsetTimeOriginal`), `ExifVersion` 0232, and
`ColorSpace` sRGB for sRGB and Uncalibrated for anything else, since
the tag has no word for P3 or Rec.2020 and the ICC profile is the
authority. Blank `Artist`, `Copyright` and `CameraOwnerName`, which
rawler copies as empty strings, are left out. Nothing says what the
edit did; that is the other half of the roadmap item and waits on
deciding what an edit's output should say about itself.

**How.** `greycard_core::exif`, on rawler's TIFF writer, as the DNG
is: rawler's `write_exif_tags` fills the root and EXIF directories
from its `RawMetadata` (and writes the GPS directory itself), then the
export's own tags go over them. `exif::payload` renders the
directories as a bare TIFF structure, which is what a JPEG's APP1
segment and a PNG's `eXIf` chunk both carry, and the image crate's
encoders take it through `set_exif_metadata` (JPEG prefixes the
`Exif\0\0` header itself). The image crate's TIFF encoder takes no
tags, so `exif::write_rgb16_tiff` writes the 16-bit TIFF whole: strips
of 256 rows, LZW, the ICC profile as tag 34675, the EXIF directories
in their own IFDs. LZW without a predictor made the R6 II's 144 MB of
16-bit samples 167 MB; under the horizontal predictor (each sample
its difference from the one to the left, tag 317 = 2) the same file
is 113 MB, and the tiff crate, ImageMagick and exiv2 all read it back.
The editor's worker now decodes with the metadata and keeps it beside
the frame for the export; the CLI's `--output` TIFF and `--preview`
JPEG or PNG carry the same block. Checked on a CR3 with exiv2 (no
exiftool on this machine): every tag above where expected, the
Canon maker note not carried (rawler does not surface it, and it
would describe a file this is not).

**CI.** `.github/workflows/ci.yml`: the pre-push hook's three steps on
ubuntu-latest, stable Rust, on every push and pull request. The apt
line is lcms2, fontconfig, wayland and xkbcommon for the crates that
link them; ort fetches its own runtime at build. The ignored tests,
which want raw files and models, stay ignored there as in the hook.
Untested until the first push; what it wants that the machine has
and the runner does not will show then.
