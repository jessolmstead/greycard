# §446. The fill's chroma mottle

*2026-10-28.* The four-step remove (Fast) leaves a green-magenta
mottle in its fills, one to two latent cells across (8 to 16 px in
the model's output), that the picture round the hole does not have:
read as the chroma's residual after a 2 px box, at distances of 4, 8,
and 16, a wall's V axis sat at 1.2 to 1.4 times the surround's, a sky's
at 1.7 to 2.8, foliage at 1.5 to 1.9. Best (25 steps) is nearly clean
on a wall and leaves a little on a sky. More Fast steps were measured
and refused: 12 steps left the sky worse (2.1 to 2.3), darkened the
foliage 18 percent, and cost 3.4 times the time.

## What we do

After the level across the edge (§443) and before the grain (§398),
the fill's two chroma axes (the denoiser's orthonormal luma and
chroma, in linear light) are smoothed by a guided filter (He, Sun,
and Tang, ECCV 2010, reimplemented) with the fill's own luma as the
guide, radius 12 model px, over the hole's pixels alone: the box
means carry a count table, so the rim a removed object leaves is
never averaged into the fill. The filter's regularization is the
ring's luma residual at distance 2, squared, so grain-sized luma is
not taken for an edge. How much of the smoothed chroma is taken is
solved per axis in closed form, the least share that brings the
fill's reading at 4, 8, and 16 down to the ring's (a band 8 to 48 px
out, set back past the rim as §443's is); a fill whose chroma already
matches takes none and is bit-identical to before, and the luma is
never touched. The work is limited to the hole's box padded by the
ring and the farthest reading, 0.006 s at 512 and 0.06 s at 1024.

Measured (V, blurred, at 4, 8, and 16, as a ratio to the surround):
the wall 1.22, 1.37, 1.42 to 0.86, 0.99, 1.05; the sky 1.67, 2.08, 2.79
to 0.74, 0.79, 0.93; foliage 1.94, 1.46, 0.87 to 0.98, 0.80, 0.59; a
sky-and-foliage edge 1.28, 1.34, 1.09 to 0.95, 1.01, 0.87. The shares
taken (U, V): the wall 0 and 0.33, the sky 1 and 1, foliage 0 and
0.51, the edge 0 and 0.28. Best on the sky came to 0.79 and 0.75. A
fill cache entry is remade once, since a cached fill must equal a
fresh one.

## What was tried and refused

A scale gate (smooth only where the mottle reads at the latents'
scale and not above) let the wall's 32 px blotches through at 1.43.
A minimax share across the three distances lifted the sky 0.01 to
0.03 and gave back correction elsewhere, for nothing; the least
share that reaches the ring at every distance stayed.

## What it cannot tell apart

The guide is the plain sum of the linear channels, so an edge between
two colors of the same sum has no edge in the guide and is softened
across a triangle about 24 model px wide on each side; a test pins
that. Round foliage the regularization is the texture's size, so luma
steps up to that size do not hold the chroma either. A fill made at a
reduced scale reads its ring in the downscaled picture and ends
smoother than its surround, the sky about three quarters of it. Luma
mottle is not touched, and holes under 9 model px are left alone.
