# §398. The frame's grain under a fill, and a swept store

*2026-10-23*

§382 fills a deep retouch hole at a reduced scale and brings it back
smooth where the frame around it has grain; §380 named a grain pass
as what that wants. This is that pass, and the cleanup of the model
file §382 retired.

## Where the noise is measured

The pipeline's own estimate (`develop/noise.rs`, variance a·level+b
per channel on the mosaic, before white balance) is not usable at the
fill's stage: the fill runs on the working image, after demosaic, the
denoise, and the matrix, and after a denoise the mosaic model would
add more grain than the frame has. So the estimate is made again on
the working image itself, with the same blocks, residual at distance
two, bins by level, and the quietest quarter of each bin, over the
window's known pixels around the hole. The residual at distance two
measures the variance at high frequency, so the amplitude matches
there and the added grain is a little finer than the frame's, whose
grain the demosaic and the denoise correlate across neighbors.

## On the denoiser's axes

A first draw was one independent normal per channel. The reviewer
pointed out that working-image grain is strongly correlated between
channels: with channel variance V, correlated grain has luma variance
3V and no chroma, while independent draws give luma V and chroma 2V,
58 percent of the frame's luma amplitude plus colored speckle the
frame does not have. The estimate and the draw now run on the
denoiser's own orthonormal luma and two chroma axes, binned by the
pixel's mean level, and rotate back. On a 24 MP frame at ISO 500
with a 480 px hole in flat sky, the hole's high-pass spread came back
at 96 percent of the surround's on luma and 91 on chroma, where the
per-channel draw gave 70 and 156.

## What is added, and where

Only the shortfall: the fill's own leftover noise is measured inside
the hole and max(0, frame − present) is added at each pixel's level,
under the hole's cover alone, so the retouch's blend feathers it with
the hole's edge. The draw is seeded by the patch's id and the pixel's
place, so a re-run, the export, and the viewport agree; the fill
cache keeps the grained result. It runs only for a fill made below
full scale.

Smooth highlights past one (reconstruction, a clipped channel through
the matrix) are nearly noiseless and would land in the top bin and
drag the line's slope to zero; blocks reaching 0.99 of a channel's
brightest known value are dropped, and the line is fitted a second
time without blocks quieter than a sixteenth of the first fit at
their level.

## Not done

- A surround that is all texture (grass, water) has no flat quarter
  at a level, so texture is read as noise and added, worst on a
  denoised frame. The cap is the develop's mosaic model mapped through
  the gains and the matrix, which the base does not carry today; a
  roadmap line.
- The second fit's floor could, in that same case, discard a quiet
  dark bin; the guard is to apply the floor near the channel peaks
  only.
- At a feathered edge the blend's grain dips to half at mid feather;
  inherent to the blend.

## The swept store

§382's model replaced the Carve LaMa, whose 208 MB file sat unused in
every older store. The store sweeps a retired file on the app's start,
on a background thread, when its name, size, and SHA-256 all match,
then the empty directory and its license. A first version swept on
the store's open, which every test that opens the user's store would
have run against the real cache; the reviewer caught it.
