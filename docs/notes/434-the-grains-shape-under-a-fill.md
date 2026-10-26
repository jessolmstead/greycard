# §434. The grain's shape under a fill

*2026-10-26*

On a frame the learned denoiser ran on, a fill came back grainier than
its surround. On the night sky (ISO 1000, +4 EV, radius 0.1 Fill), the
hole's luma high-pass spread over a ring round it was 2.77 at distance
one and 1.49 at two. The denoiser's leftover is correlated, in soft
streaks, with little energy at distance one. §398 drew white grain
matched at distance two, so the finest scale got far too much. This
continues §398.

## The shaping

The estimate now reads the residual at distance one as well as two, in
the same blocks. The level-binned fit at two still sets the amplitude.
The shape is read from the blocks the second fit keeps (the same
QUIET_FLOOR test) whose residual at two is within 1.5 times the fit at
their level, since texture reads coarser than grain. Their variances at
one and two are summed per level bin, the fit's own bins. The ratio used
at a level pools that level's bin and its two neighbors, where the
amplitude and the shortfall are read, and falls back to every bin when
fewer than 8 blocks of a group lie in those three. The luma's ratio is
read on the luminance (Rec.2020 weights), not on the denoiser's
equal-weight luma axis: after a demosaic the green is finer than the red
and the blue, and the eye leans on the green. On the night sky without a
denoise the axis read 0.77 where the luminance and the export read about
0.85. The chroma's ratio is read on its two axes together.

The shortfall is taken at both distances. At the hole's mean level, the
variance the frame has at two less what the fill carries, and the same
at one (each through its own ratio), give the added grain's ratio. A
model's fill carries coarse grain of its own (ratios of 0.33 to 0.47 on
both frames below), so what is added over it is finer than the frame's
grain, to make up the fine scale. Two edges: a fill with more energy at
one than the frame has to spare gets the widest draw (3 px), and a
shortfall finer than white is drawn white and stays a little short at
one.

Each axis is drawn white over the region and a margin, from the seed and
the frame position as before. It is smoothed by a separable Gaussian
whose width is solved by bisection so white noise through it has that
ratio. The ratio is closed-form in the kernel's autocorrelation a: the
residual at d has variance 1.25 − 2a(d) + a(2d)/4 + a(d)²/2. The draw is
then scaled so its residual at two has the shortfall's variance. A ratio
of 1 or more draws white.

We chose one and two over one and four. Two is where the amplitude is
fitted, and at four a 16-pixel block picks up the scene's own structure.
On correlated test grain, four follows within 15 percent.

The fill cache's version goes to 2, so fills kept on disk are made again
with the new grain rather than keeping the speckle.

## The night sky, where this started

ISO 1000, +4 EV, radius 0.1 Fill. Hole over ring, display luma, 8-bit
PNG, pooled:

| develop | d=1 | d=2 | d=4 | d=8 |
|---|---|---|---|---|
| no denoise, before | 1.30 | 1.04 | 1.17 | 1.06 |
| no denoise, after | 1.07 | 1.03 | 1.16 | 1.06 |
| profiled, before | 1.02 | 0.88 | 1.02 | 0.95 |
| profiled, after | 0.81 | 0.89 | 1.04 | 0.96 |
| learned Best, before | 2.77 | 1.49 | 1.50 | 1.35 |
| learned Best, after | 1.79 | 1.54 | 1.62 | 1.45 |

This frame is a poor judge. It also shows two regressions: profiled at
d=1 went from 1.02 to 0.81, now under, and learned Best got slightly
worse at the coarse scales (d=2 1.49 to 1.54, d=4 1.50 to 1.62, d=8 1.35
to 1.45). Both come from how this frame is measured:

- On Best the fill sits at the frame's linear level (0.000283 against
  0.000274) but displays 50 percent brighter. In the deep blue sky the
  display green clips at zero in 83 percent of the ring's pixels and
  in 13 percent of the smooth fill's, so the fill's green noise reaches
  the luma and the ring's does not. Coarser grain carries more of that
  unclipped green at the coarse scales.
- Profiled, the ring holds stars, which lift its pooled spread about 10
  percent over the per-block median at its level. The 1.02 was white
  grain too fine, read against a ring too high. Per block, the
  surround's ratio of one to two is 0.79 and the hole's 0.72.

## The overcast sky, the measure that holds

ISO 500, −1 EV, so every channel in the ring stays between 0.29 and
0.90. Radius 0.1 Fill, with the ring kept to the sky. The export is
16-bit TIFF, since the grain is under one 8-bit level. The pooled ring
spread is ruled by the sky's gradient and birds (0.3 per block at d=8
against 1.0 pooled), so the measure is the median over 16-pixel blocks
of each block's residual spread, hole over ring:

| develop | d=1 | d=2 | d=4 | d=8 |
|---|---|---|---|---|
| no denoise, before | 1.16 | 1.02 | 1.09 | 1.08 |
| no denoise, after | 1.01 | 1.02 | 1.09 | 1.08 |
| profiled, before | 3.33 | 3.10 | 2.48 | 2.58 |
| profiled, after | 3.10 | 3.11 | 2.45 | 2.58 |
| learned Best, before | 1.39 | 1.35 | 1.29 | 1.30 |
| learned Best, after | 1.32 | 1.35 | 1.28 | 1.30 |
| the fill alone, no denoise | 0.39 | 0.65 | 0.73 | 0.78 |
| the fill alone, profiled | 2.61 | 2.87 | 2.30 | 2.39 |
| the fill alone, Best | 0.74 | 1.08 | 1.02 | 1.07 |

On the overcast sky without a denoise, the hole now matches at every
distance. On Best, the hole's excess is the same at one, two, and four,
so the shape is right and the amplitude is not. The fill alone already
carries the ring's spread at two, yet the estimate reads the frame at
1.22 times what it reads in the fill, and adds the difference. Profiled,
the fill alone is three times grainier than the denoised sky, and the
estimate reads the frame at about the fill's. Both are §398's open item:
the window holds sea and rock at the sky's level, and on a denoised
frame their texture outweighs the grain in each bin's quietest quarter,
so the frame reads noisier than its sky. Profiled, the fill's own grain
would need to come down, which adding cannot do.

We tried two other routes on the overcast sky. Taking the surround's
ratio as the per-block median rather than pooled gave 0.65 either way,
and the table did not move. Before the shortfall rule, drawing the
frame's shape over the fill's coarse grain gave Best 1.07 at d=1 and no
denoise 0.87, because a coarse draw over a coarse fill is short at the
fine scale. We did not build a two-scale draw: the remaining Best excess
is flat across the distances, and a different shape cannot change its
level.

The shaping costs one more residual pass in the estimate and a separable
blur per axis over the region, 5 to 7 taps at the widths seen: a
fraction of a second against the fill's seconds.

## Not done

- The amplitude on a denoised frame whose window holds texture at the
  hole's level: the cap §398 names (the develop's mosaic model through
  the gains and the matrix), or a fit kept to blocks like the ring's.
  A roadmap line.
- A fill grainier than its surround is left as it is; bringing it down
  is a roadmap line.
- The per-channel structure (green finer than red and blue) would need
  the grain's full cross-spectrum. We keep a draw on the axes with one
  width each.
