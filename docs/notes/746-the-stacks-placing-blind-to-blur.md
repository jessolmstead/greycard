# §746. The stack's placing, blind to a difference in blur (2026-11-25)

A focus stack's frames are placed link by link, each onto its
neighbor, by a gradient fit of a similarity (§612). We took the
softness away from the center for focus breathing left unrecovered.
It is not. The fit reads part of the difference in blur between two
neighbors as motion: a shift, a turn, a change of scale, much the
same at every link, since each pair sees the scene with the focus
one step on. So it adds up along the chain. On a bed of 31 frames
with no breathing at all the chain made 1.7% of scale and a third of
a degree, and ended 10 px out at the corners.

**The refinement.** A symmetric blur does not move the phase of a
patch's cross-power spectrum. After each gradient fit, patches of 32
and 64 px on the first level of 2048 px or under, in linear light,
each over its own mean and under a Hann window, give their shifts
from the phase's slope; a similarity is fitted to them robustly, and
the passes repeat until two in a row move the corners under 0.05 px.
A patch weighs by what its slope can tell and by its phase's
coherence, squared; one with clipped samples is left out. Only the
stack asks for it (`Refine::Phase`); the bracket and the panorama
place as they did. The residual that gates a link is taken again at
the placing used.

**The guard.** Two frames a similarity cannot relate (a hand moved
between them, with near and far apart) give a field of patches the
fit cannot hold, and the refinement then lands wherever its heaviest
patches pull: on one such pair of real frames, 12 px from the
gradient's answer and worse by every check. So the refinement is
kept only where it settles within sixteen passes, the patches'
weighted median miss is under 0.4 px at the working level, the plain
median under 3, and the weight is spread over the worth of eight
patches or more, so that no one bright patch carries it. Otherwise
the gradient's answer stands, as before.

**Measured.** With a known answer, on real texture blurred and moved
by known amounts, the worst corner of a 24 MP frame:

| Blur between the frames | Gradient | Refined |
|---|---|---|
| Gaussian, sigma 3 | 0.59 px | 0.08 px |
| Disc, radius 6 | 0.79 | 0.09 |
| Cat's eye | 0.53 | 0.07 |
| Ramps, one focus step | 0.55 | 0.17 |
| Ramps, far apart | 2.0 | 0.55 |

A burst of 31 copies of one frame at one focus, each moved by a
known amount, chained: 0.36 px by the gradient, 0.07 refined; the
refinement adds no drift of its own. On the beds, the worst corner
of 11 frames falls from 2.9 to 1.0 px with breathing and from 3.1 to
1.6 without, of 31 from 10.4 to 2.0 on the depth bed, and no bed is
worse than the gradient alone. The faint texture of §740 kept rises
with it. A link costs 0.3 to 0.4 s more at 24 MP.

**What the guard gives up.** Stacks of two to five frames, and
coarse steps, are mostly turned away: no gain, no loss. A chain may
mix kept and refused links; each kept one puts a smaller bias of the
same sign for a larger. A repeating pattern of 8 to 13 px at the
working level, with the gradient's answer more than half a period
off, settles a period out. A stack that is mostly blur is still 10 px
and more out.

No focus stack of real frames at one exposure has been through this.
The real pairs at hand showed only that the guard turns the unfit
ones away.
