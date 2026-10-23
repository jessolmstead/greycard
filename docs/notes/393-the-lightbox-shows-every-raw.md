# §393. The lightbox shows every raw: the ORF preview, a develop where there is none, the pair's JPEG

*2026-10-22*

A tester's OM-5 files (issue #36) showed nothing in the lightbox
unless they had been edited, while the Ricoh DNGs beside them showed
fine. Three things were wrong, each on its own.

## rawler reads the ORF preview

An ORF carries a large JPEG (3200x2400 on the OM-5) and a 160x120
thumbnail in its makernote, pointed at by the CameraSettings
sub-IFD's PreviewImageStart and PreviewImageLength, offsets relative
to the makernote as the Equipment and ImageProcessing sub-IFDs are.
rawler's ORF decoder parsed neither, so `preview_path` gave None, and
the lightbox called that "no camera preview in the file". Our fork now
parses CameraSettings and implements `preview_image` and
`thumbnail_image`; sent upstream as dnglab/dnglab#880. CameraSettings
reuses tag numbers of ImageProcessing (0x0100, 0x0600), so it is kept
in a field of its own rather than registered as a sub-IFD, where
`find_ifds_with_tag` could have read the white balance from it. A
bad pointer, a thumbnail past the end, or a preview past the end
logs and gives None; the raw still decodes.

Thumbnails and the loupe gain the same preview, since every camera
picture goes through `preview_path`.

## A develop where there is none

rawler has no preview reader for several formats besides, and a
camera may write none. The lightbox and the loupe now develop a quick
2048 picture through the thumbnail pool's own developer when a raw
gives no camera picture, behind a `develop` flag on the want that the
develop view's stand-in leaves off (its real develop is already on
the way). The picture is labeled as the local preview, never as the
camera's JPEG, and is kept as a local preview so it is made once. The
in-flight check between wants is one-way: a want that may develop
covers the same want that may not, not the reverse. A picture that
will not read (a broken JPEG or TIFF) still fails, and is not
developed.

## The pair's JPEG in the lightbox

The culling loupe attaches the JPEG beside a raw to its want, and
`decode_paired` takes the JPEG unless the raw's own preview is larger.
The lightbox built its wants without it, so a raw+JPEG pair showed
the raw's preview there, and an ORF nothing. The lightbox's file wants
now carry the companion as the loupe's do.

## Not done

- Other formats rawler reads no preview for still take the develop;
  each is a small upstream change like this one, as a tester reports
  one.
- A loupe and a lightbox want of one file at once can develop it
  twice, since the in-flight check is per prefetcher; the second
  costs a develop and nothing else.
