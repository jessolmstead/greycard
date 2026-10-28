# §447. The fill's level read by sector

*2026-10-28.* §443 fitted one plane over the whole band round the
hole with a step per sector. On a frame where the hole crosses from a
plain sky into a dark hill the plane bent to meet the hill, and the
sectors at the hole's sides read their step against a plane halfway
between the two, asking for gains of 0.6 that the picture does not
have; the gate stepped aside and all three fills, Patch, Fast, and
Best, stayed light. Read at the edge, the sky asked for 0.97.

## The fit

One tilt is shared by every sector; each sector has a level of its
own on the picture's side and a step on the fill's. Three Huber passes
keep a sector whose band is nowhere near a plane from setting the
tilt. Each sector then gets a sigma, the spread of its residuals over
its level with a floor of 0.05 (two sigma is §443's tolerance on a
smooth sector), and a weight of its share of the edge over sigma
squared, and the fit is made once more with those weights, so a
textured or crossing sector has little say. Sixteen sectors: with
eight the scatter has five or six degrees of freedom, and a fill with
its own structure came within a hair of being matched.

## The gate, the error, and the shrink

Agreement is read on the step's luma, within two sigma of the
weighted median, three quarters of the weight. The held steps' mean
carries a standard error from their scatter, with the effective count
from the weights; and since a texture at the hole's own scale differs
between the fill band and the picture band coherently all round the
ring, a bias the scatter cannot see, the same read is made on the
picture alone across three pseudo-edges, the hole grown by 8, 12, and
16 px, and the RMS of what they read is added to the error in
quadrature. The mean is then shrunk by the square of its error over
itself (a positive-part James–Stein shrink), the three channels scaled
by the luma's kept share: an ask well above its error is taken nearly
whole, one within its error is left alone, and a level read by chance
moves the fill little. A hard gate at 1.5 errors was built first and
refused; it passed a correct fill one time in eight and then applied
the whole mean. A soft threshold, the ask less one error, kept only
61 and 68 percent of the mast's ask; the shrink keeps 80 and 89.

## Measured

The fill at the edge over the picture, luma, 1.000 a match: the mast
Fast 1.026 to 1.003, Best 1.032 to 1.001, Patch 1.001 left alone; the
wall's first fill 1.019 to 1.030, since its stones read as texture at
the pseudo-edges' radii and the match keeps 68 percent of its ask
there; its other three, which §443 moved by two percent or less, now
left alone, about a percent worse at the edge; the sky over a
textured ledge left alone as before. Over sixty draws of grain from 2
to 10 percent, a fill already at the picture's level moves 0.7 percent
at most and a fill 3 to 8 percent light is taken every time within
1.1 percent. A perfect fill swept through every phase of a pattern:
at a 63 px period master moved it at 32 of 63 phases up to 9 percent
and this moves it at 8 up to 1.4; at 31 px, 10.2 percent against
master's 13.9; at 44 px, 6 of 44 phases up to 5.9 where master moved
none. The horizon and wall-edge cases are refused by agreement, a
texture taken up out of phase by the error. A blotchy wall now takes
a true level, within 0.02 at one phase. The match runs once per fill
after the model, 150 ms in release on a 1024-square window with a
400 px hole, 33 ms on the mast's Fast window.

## What remains

Patterns with a period of 30 to 45 px can still move a correct fill
by 6 to 10 percent at some phases: the pseudo-edges sample 8 to 16 px
out, a quarter of such a period, and the window's 32 px margin keeps
them from going further; a known-gap test pins it. A long thin stroke
is still never matched. The mast's sky still sits 1 to 1.5 percent
light 20 to 40 px in while its edge matches: the fill's own bulge,
which an edge match cannot see.
