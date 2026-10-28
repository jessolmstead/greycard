# §443. The fill's level across its edge

*2026-10-28*

A tester saw a generative fill come back lighter than the sky around
it, mostly with Fast. On a flat stone wall we measured the hole at
1.14x its ring in linear light in the Fast model's own output and
1.20x after the look; Best read 1.10x. §414's ring match compares the
decoder's answer with the picture where both are known, so it corrects
the decoder and cannot see a level the UNet set inside the hole.

## The match

`remove/seam.rs` runs inside `leveled` after the ring match, on every
path through `composite`, Fast and Best, native and scaled. One plane
in linear light is fitted to a band of the picture set back 8 px from
the hole's edge and running to 16 px, and a band of the fill 4 px
inside it, the fill's side raised by a step in each of eight sectors
round the hole's middle. The plane is the picture's own gradient; the
steps are the fill's level. A level the UNet set is the same step all
round, give or take the texture, so the match asks for agreement: a
sector agrees when its step is within 0.1 (in logs) of the median, at
least three quarters of the edge must agree, at least five sectors
must hold enough band, and the median must sit within 0.8 to 1.25.
Then the edge-weighted mean of the steps, each held within 0.1 of the
median, is multiplied into the whole fill, which leaves its texture as
it was. Otherwise the fill is left exactly as the ring match gave it,
with a line in the log saying what the sectors asked for.

The set-back band is what makes it safe: a removed object leaves a
rim in the picture (an anti-aliased fringe, a halo, a contact shadow)
that the hole's coverage does not take in, and read from the edge the
rim set the whole fill's level, a 1 px fringe pulling a correct fill
down by a tenth or more. The agreement gate is the other half: a hole
whose edge runs along a horizon or a roofline, or a fill that took up
the wall's blotches out of phase, makes the sectors disagree and is
left alone, where a single clamped fit pushed it to the clamp.

Two earlier shapes were dropped on the way. Local quadratics along the
edge: their `u² + v²` term tells a ring inside the edge from one
outside, which is the step itself, so they absorbed part of the level.
A smooth gain field carried inward by a Laplace solve: on the wall the
sectors scatter by a tenth from texture alone, and a field painted
that scatter into the fill.

## Measured

The step at the edge, 8 px inside over 8 px outside, linear:

| fill | Fast before | Fast after | Best before | Best after |
|---|---|---|---|---|
| wall, large hole | 1.12 | 1.04 | 1.08 | 1.01 |
| wall, three small holes | 0.99 to 1.04 | 0.99 to 1.02 | 1.01 to 1.03 | 1.01 to 1.03 |
| plain sky | 1.07 | 1.00 | 1.05 | 1.00 |
| sky across a horizon | 1.05 | left alone | 1.03 | left alone |

The horizon case is wrong content, a washed-out sea, which no level
can fix. A 1024² window with a radius-400 hole takes about 60 ms.

## Not done

- A long thin stroke (a cable on a sky) is never matched: the sectors
  are angles about the hole's middle, so four of the eight starve.
  Sectors in the hole's principal-axis frame, or by position along the
  edge, would take it.
- A blotchy wall, patches at the hole's own scale, refuses a true
  level: one plane cannot follow them, and the gate fails on the safe
  side. Fine stone texture passes.
- A contact shadow wider than the 8 px gap still leaks in: 0.6% at
  12 px, 6% at 20 px.
- With one gain a fill, the middle of a large sky fill is still about
  1.5% light.
