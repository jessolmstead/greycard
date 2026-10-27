# §438. The grain's draw held to the shortfall

*2026-10-26*

A fill on a frame the learned denoiser had run on came back as a patch
of colored blotches, from both models, in every small window. The
model's own answer was clean; the cached fill after §434's grain was
not. §434 smooths its draw to the surround's shape and then scales it
so its reading at distance two matches the shortfall's. The kernel
keeps a unit field's variance, so that scale is the boost a smoothed
field needs to read as white at two: for the widest draw, 3 px, about
60 in variance, 8 in spread. On a frame whose residual is small because
the denoiser took the noise out, not because its grain is wide, the
shape still reads wide (0.55 at one over two round the hole, the draws
at 3 px), and the boost paints the hole with blotches eight times the
grain. Nothing measured them: the estimate after the draw read 0,
every block past the texture ceiling.

The draw's gain is now held to 1: a draw carries the shortfall's
variance and no more, read short at two when it is wide. The synthetic
test with truly wide grain round the hole now pins the shape and that
the draw never passes the surround, in place of a match at every
distance; the fine-scale test reads 9 percent short at one. The
night-sky and overcast harnesses drew at under 1 px, where the gain
was near 1, so they are as §434 measured them.

The bound this stands in for is a measured one: the surround's
variance within its quiet blocks, about their mean, which a wide grain
carries and a denoised wall does not. Carried per level bin beside the
shape, it would let a truly wide grain be drawn in full and still hold
a denoised frame's draw down. That is the roadmap's line.
