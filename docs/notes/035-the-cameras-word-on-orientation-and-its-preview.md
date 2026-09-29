# 35. The camera's word on orientation, and its preview for the filmstrip (2026-09-07)

Two things came out of the filmstrip ask in §34's addendum.

**Orientation was never applied.** rawler's `RawImage::new` leaves
`orientation` at `Normal` with a TODO beside it, and the backend took
that field, so every portrait frame developed sideways and every edit
on one was made on the sideways picture; the bridge frame in the
screenshots since §14 is a portrait. The EXIF block rawler reads does
carry the tag, so `decode_source` now takes `metadata.exif.orientation`
through `Orientation::from_exif` (the enum is in tag order, tested)
whenever the image's own says Normal. `develop::orient` then turns the
picture as it always could. The DNG writer's tag follows, since it
writes the frame's orientation.

Consequence to know: a sidecar made before this on a portrait file has
its shapes, crop and turns in the sideways picture's units. There are
no such sidecars outside this machine; here the test files' edits are
scratch.

**The filmstrip reads the camera's JPEG.** `decode::preview_path`
asks the decoder for its preview image (the thumbnail as the
fallback), decodes it to 8-bit sRGB, and returns it with the EXIF
orientation. The worker's thumbnail box-downscales that to the strip's
width and turns it through the same `develop::orient` as the developed
picture, so the two agree; the demosaic path remains for a file with
no preview. Canon's "big" preview is the full frame (6000×4000, 8192×
5464 on the R5 II), so a thumbnail costs 40 to 110 ms where the decode
and bilinear develop cost over a second. A folder fills in as fast as
the strip can show it. The camera's rendering is not ours, which is
right for a strip: it is what the camera showed, and the viewport
shows the develop.

### Made rasters kept on disk (2026-09-07)

A learned raster is now written as an 8-bit PNG under
`~/.cache/greycard/masks/`, named by a hash of the file's path, the
model's id and the shape's JSON, and read back before the model is
asked. The key is what could change the answer: another file, another
model, or another prompt. The base develop is not in it, by the same
reasoning as the in-memory cache (a subject does not move with the
white balance). A raster whose height does not match the file's aspect
is ignored, so a crop in the develop's pipeline could not hand back a
stale shape. The cache dir rather than beside the sidecar: no clutter
in the user's folders, and `--no-sidecars` stays honest; a file moved
elsewhere is found again, at the cost of one model run. Opening the
bridge frame a second time gives the same screenshot to the pixel and
skips the 3 s.
