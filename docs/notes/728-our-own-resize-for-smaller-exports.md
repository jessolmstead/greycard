# §728. Our own resize for smaller exports, with no clamp (2026-11-21)

An export made smaller went through the image crate's Lanczos3, which
holds a float to 0..1. That is right for a display picture and wrong
for ours at that point: the picture is still scene light, and the
planes beside it are signed. Three faults came from the one clamp, all
since the planes first went through the resize:

- light over one was cut, so a frame pulled down in exposure kept
  highlights at full size that came out flat in a smaller export;
- a mask's Texture, Clarity, and Structure lost their negative half;
- a mask's Sharpen did about a twentieth of its work, the clamp aside,
  because a resized delta of stops is a mean of stops, close to none.

A user saw the last as "Sharpen in a mask does nothing". In the
viewport it works; at Fit it is as slight as the picture's own
sharpening is there.

**The resize.** `greycard-render::lanczos` is a port of the crate's
Lanczos3: its kernel, window, weights, pass order, and the order of
each sum, so a picture that stays inside 0..1 comes out to the bit
(tested against the crate, 1-pixel axes and a 230 times reduction
included). Ours keeps every value, takes one or three channels, and
runs its rows in parallel: a 45 MP frame to 2048 in about 60 ms where
the crate took near 800, at a lower peak. The picture, the range
masks' sample, and the bands all go through it. A first cut kept the
crate and carried the bands on a pedestal; one resize that does not
clamp is less code and serves the picture too.

**A mask's Sharpen made smaller.** The delta for a smaller export is
not the delta resized: it is the stops between the two resized
pictures' luminance, sharpened and not (`fitted_delta`), each held at
the sharpen's own pedestal first. Without the floor two small
luminances give a wild ratio and dark noise comes out as bright
colored dots. After an output sharpen the delta is made again from
both sides output-sharpened. It is exact in luminance; a share
between the ends strays per channel.

**What changes for a picture with no mask.** With Output Sharpen off,
a smaller export of a frame inside 0..1 is the same to the bit. With
it on, the sharpen now sees what the clamp hid: beside a blown
highlight the old export cut a dark halo, because the clamp kept the
clip from the sharpen's clip guard, and that is gone; under black the
resize's ring reaches the sharpen. Both are what the full-size path
already did.

**Previews and caches.** The edited previews' last fit uses the
export's path, and `EDITED_RECIPE` and `REDUCED_RECIPE` are bumped, so
edited previews are remade once. `DEVELOP_RECIPE` is not: it names the
learned masks' rasters, and a bump would run every Sky, Subject, and
Parts mask again for nothing. The masks' base picture and camera
match's renders go through this resize too and change for a frame with
light over one; neither is keyed by it.

Left: the viewport at Fit applies the delta's mean to the picture's
mean, the right size and not exact; a frame too large to hold is cut
smaller through the geometry before the fit, so its delta is a
resampled one.
