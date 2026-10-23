# §408. The decoder in tiles

*2026-10-23*

§406 left a tiled VAE for 8 GB cards. Built and measured, the item
narrows: the decoder tiles well, the encoder does not, and the UNet
sets a floor the VAE cannot lower.

## What tiles

At 1024 the decoder runs as nine 512 tiles, three a side at 0, 256,
and 512, through the 512 export in one session; each tile is
weighted by a linear ramp across its 256-pixel overlap with its
neighbors, the weights summing to one and the result divided by the
total regardless. Against the whole decoder the answer agrees to
47 dB in the hole and 45 outside, 0.34 dB off the torch reference,
and the difference images show content, no line at a tile edge;
the decode takes 1.27 s against 0.65, about 4 percent of a removal.
Tiled is the default at 1024; an environment variable runs the
decoder whole for a comparison.

The encoder was tiled the same way and taken out again: 3.8 dB
worse in the hole with no seam, because each tile's group norms see
one tile and the UNet spreads the small latent shift into other
detail; a wider overlap did not help. Its whole 1024 peak sits
under the UNet's anyway, so tiling it saved nothing.

## What it buys

Alone in a process the peak is the UNet's 6.8 GB either way. In the
editor, with the denoiser's buffers cached in the shared context,
one removal peaks 8.0 GB over the desktop tiled against 9.2 whole,
and three removals 8.6 against 9.5. So the roadmap's "an 8 GB card
runs the 1024 window" is not met by this and cannot be by the VAE:
the UNet's 6.8 GB is the floor, and the rest is the buffer cache
(§406 kept the default for the other models' sake). An 8 GB card
gets the 512 window, which §405 already chose for it, and a 12 GB
card gets a gigabyte of room back.
