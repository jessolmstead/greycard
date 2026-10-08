# 260. The camera match keeps lightness off chroma (2026-10-07)

The user turned on the fitted R6 Mark II Faithful look over a finished
edit of a portrait against fall foliage, and the picture "blocks up
and looks kind of deep-fried". At 1:1 the bark and the coat went
mottled, the skin picked up a speckle and the hair edges crunched.
The edit was not to blame. The table turned the picture's chroma
noise into lightness.

**What the table did.** We measured the table's Jacobian in Oklab on
both sides: how far its output lightness moves for a step of its
input's a and b. A look that keeps lightness a matter of lightness
scores near zero there; identity scores exactly zero. This table
scored 1.4 to 2.0 on the bark's colors and 1.9 on the coat's, so a
chroma wobble of 0.01 came out as a lightness step of 0.02, a visible
one. Three checks put it on that slope and nothing else:

- Smoothing the table (a Gaussian over its nodes, sigma one and two)
  left the mottling as it was, so it is not bumps from cell to cell.
- Blurring only the chroma of the render before the table, lightness
  untouched, took the speckle away.
- Rendered through the real export, fine lightness texture on the bark
  (the standard deviation left by a 9-pixel box blur) was 0.0047 with
  no look and 0.0092 under the table.

Every fitted table we measured had it: seven bodies and styles, under
both curves. Over a grid of the common colors (Oklab lightness 0.3 to
0.92, chroma under 0.12), their mean was 0.40 to 0.93 and their 95th
percentile 1.3 to 2.8. The R7 and R8 Faithful tables are the R6 Mark II's rows
under their own headers (they borrow, §238), so they had it too.

**Where it came from.** It came from the residual table. On the R6
Mark II Faithful frames under AgX, the matrix alone scores 0.15 and
the matrix with its curves 0.17; the table takes it to 0.61. What the
table adds is small. It moves lightness by 0.005 on average where the
data is (95th percentile 0.017), a quarter of a visible step. But it
puts those moves across short distances in chroma, and a small move
over a short distance is a steep slope. Held out over every frame
those moves buy nothing, as the numbers below show. Two causes we
expected are not it:

- **Not the exposure match.** Taking each frame's mean lightness
  error off its camera side and refitting moved the coupling from
  0.61 to 0.57. Putting the same offsets back on shuffled frames
  added nothing.
- **Not only roughness.** Raising the smoothness term to 30 brought
  it to 0.36, at the same fitted error at which the term below
  reaches 0.10.

The structure is real in the fitted data and does not carry from one
frame to the next. Where in the camera's JPEG it comes from, we have
not found.

**The fix.** It has two parts, both in `greycard-match`'s fit:

- **The table's corrections are in Oklab.** It is still indexed by
  the encoded color, as a `.cube` must be. But what it adds goes onto
  the Oklab of what the matrix and curves give: lightness, a and b.
  So lightness is one of its three coordinates, and the data term
  is fitted in the units ΔE is measured in. With nothing else
  changed this fits as the encoded table did, held out 0.0160 on the
  R6 Mark II Faithful frames either way and 0.0180 in the oracle
  test either way.
- **A flatness term on lightness.** At every node it penalizes the
  gradient of the whole model's output lightness (the matrix and
  curves' and the table's together), with its part along the input's
  own lightness taken off. Lightness may follow lightness freely;
  following chroma costs. That direction comes from the lattice's own
  forward differences of the input's lightness, not the analytic
  gradient. Otherwise the lattice's discretization error would be
  penalized as if it were coupling. Near black, where Oklab's cube
  root bends hardest, that pushed a plain identity off by more than a
  test allows. The weight is relative to the same one-vote-a-node
  data term as smoothness and pull: `LutParams::flatness`, 32.
- **Over the common colors only.** The term holds whole up to an
  input chroma of 0.12 (`FLAT_CHROMA`, the range the coupling is
  measured on; the fitted sets' blocks reach 0.11 at their 99th
  percentile) and fades to nothing at 0.15 (a smoothstep). Without
  the fade, the term reached colors no frame showed. Because the base's lightness is in it, it cancelled the
  matrix's own lightness against chroma everywhere. On the oracle's
  fixture, colors past chroma 0.2 moved in lightness by 0.017 on
  average and 0.066 at most; pure red's green channel went from 0.017
  to 0.066. With the fade they move by 0.0010 on average and 0.013 at
  most, where the table without the term moves them by 0.0002 and
  0.0045. On the R6 Mark II Faithful frames, a fade ending at 0.2
  left 0.0029 and 0.033, and ending at 0.15 left 0.0005 and 0.005. In
  the common colors the fade changes nothing measurable: coupling
  0.104 to 0.106 (95th percentile 0.33) with it or without, held out
  0.0159 either way.

We tried first with the residual left in encoded sRGB and a fixed
lightness axis, the way lightness rises at mid grey. It brought the
mean down but barely moved the 95th percentile (1.9 to 1.6 at a
weight of 64), and held-out error rose. The axis is right only near
grey.

**The numbers.** The match's own sample of every group (334 frames
over 12 groups), from the `--match-compare` records, with both roots
mounted. Each group's converged pairs were fitted at four weights
and every frame held out once. Coupling is mean / 95th percentile;
held out is the sheet's figure, the mean of the frames' means. The
sweep was a scratch program over the records, run on the fit as
committed, fade and all; `--match-compare` now reports the default's
coupling beside the sheet's figures.

| group | curve | frames | 0: coupling, held out | 32: coupling, held out |
|---|---|---|---|---|
| R5 Faithful | pc | 36 | 0.61 / 1.79, 0.0154 | 0.07 / 0.26, 0.0150 |
| R5 Faithful | AgX | 36 | 0.76 / 2.33, 0.0178 | 0.09 / 0.30, 0.0183 |
| R5 Standard | pc | 28 | 0.34 / 0.99, 0.0081 | 0.06 / 0.16, 0.0081 |
| R5 Standard | AgX | 29 | 0.48 / 1.64, 0.0130 | 0.08 / 0.24, 0.0126 |
| R5 II Faithful | pc | 22 | 0.73 / 2.26, 0.0125 | 0.10 / 0.24, 0.0139 |
| R5 II Faithful | AgX | 22 | 0.79 / 2.60, 0.0145 | 0.12 / 0.29, 0.0163 |
| R5 II Standard | pc | 35 | 0.55 / 1.49, 0.0145 | 0.07 / 0.24, 0.0136 |
| R5 II Standard | AgX | 35 | 0.68 / 1.95, 0.0210 | 0.10 / 0.35, 0.0194 |
| R6 Faithful | pc | 40 | 0.50 / 1.74, 0.0104 | 0.04 / 0.12, 0.0104 |
| R6 Faithful | AgX | 40 | 0.63 / 1.89, 0.0122 | 0.06 / 0.20, 0.0127 |
| R6 II Faithful | pc | 39 | 0.57 / 1.78, 0.0126 | 0.09 / 0.32, 0.0124 |
| R6 II Faithful | AgX | 38 | 0.60 / 1.88, 0.0160 | 0.11 / 0.33, 0.0159 |
| R6 III Faithful | pc | 40 | 0.42 / 1.26, 0.0103 | 0.06 / 0.18, 0.0108 |
| R6 III Faithful | AgX | 40 | 0.46 / 1.12, 0.0119 | 0.06 / 0.18, 0.0123 |
| GFX 100S II Provia | pc | 24 | 0.68 / 1.88, 0.0208 | 0.10 / 0.28, 0.0216 |
| GFX 100S II Provia | AgX | 25 | 0.67 / 1.74, 0.0217 | 0.10 / 0.31, 0.0224 |
| GFX 100S II Reala Ace | pc | 38 | 0.42 / 1.17, 0.0122 | 0.07 / 0.19, 0.0122 |
| GFX 100S II Reala Ace | AgX | 38 | 0.51 / 1.62, 0.0143 | 0.10 / 0.27, 0.0143 |

Over the 18 rows the mean coupling goes from 0.58 to 0.08 and its 95th
percentile from 1.73 to 0.25; the held-out error from 0.01440 to
0.01457. At 16 those are 0.11, 0.35 and 0.01446; at 64, 0.06, 0.17 and
0.01468. Past chroma 0.2 the lightness moves off the matrix and
curves' by 0.0009 on average at 32 against 0.0003 at none, and at
most by 0.023 against 0.021. We took 32. At 16 the R6 Mark II's bark
still scored 0.40 and its rendered texture still rose by about an
eighth over no look. At 32 the bark scores 0.24, for a held-out cost
of 0.00017 across the groups, under a hundredth of a visible step.
The R5 Mark II Faithful, the smallest set, pays most: held out
0.0125 to 0.0139 per channel and 0.0145 to 0.0163 under AgX, a tenth
of a visible step, on 22 frames. The GFX Provia pays 0.0008 and
0.0007, and four other rows 0.0004 or 0.0005; the R5 Mark II
Standard gains 0.0009 and 0.0016. The R7 and R8 rows (three frames
each) are left out, since those bodies borrow.

On the frame that started it, rendered through the real export with
the refitted R6 Mark II Faithful table, the bark's fine texture is
0.0051 (0.0047 with no look, 0.0092 under the old table), and the
speckle on the skin is gone. Over the common colors the look's color
is the same; past them it moves about as much as the table without
the term moves it.

**Tests.** `the_default_keeps_lightness_off_chroma` holds it on the
oracle's fixture, every eighth block pair of the September R6 Mark II
Faithful set. Without the term the coupling there is 0.71 / 2.25; with
it, 0.10 / 0.31. Held out over the test's eight frames it is 0.0173
against 0.0180. Past chroma 0.2 it holds the lightness to within 0.002
of the matrix and curves' on average and 0.02 at most. Unit tests hold
the flatness operator symmetric, never negative and free along the
input's lightness, `rises` zero on the lattice's last face, and the
fade's ends. The oracle itself now runs the fit as the Python has
it, the term off, since the script never had the term and its fitted
figure is what the term trades away. The ignored sweep covers the
weight too. The editor's log line for a fitted look carries the
coupling.

**Which fit wrote a table.** Tables fitted before this keep their
coupling until the match is run again, and until now nothing on a
table said which fit wrote it. The match now declares it in the
header, `# fit: 2`, beside the display curve and the body.
`greycard_match::FIT_VERSION` is the number, with a line beside it
for each bump. It is to be bumped by any change to the fit that
changes what a table does to a picture. A fitted table without the
key is fit 1, the way a fitted table without `display_curve` is per
channel. Three places read it:

- **The Look section.** When the chosen look's table for the
  picture's curve was written by an earlier fit, it shows the line
  that a missing curve's table gets: yellow, over a Refit button that
  opens the match's sheet on that look's group. The line is
  "Fitted by an earlier version of the camera match, which can turn
  a picture's color noise into blotches of lightness. Refit it to
  bring it up to date." It never turns the look off. A table the
  match did not write is never called stale, and neither is a later
  build's.
- **What a run may replace.** A body's own fit replaces a fit of
  that body from more frames when an earlier fit wrote the old one.
  Without this, a refit that kept a frame or two fewer than the old
  table's 38 would be refused as "fitted from more frames". A borrow
  still never
  replaces a body's own fit, old or new.
- **Tests** for the reading (declared, absent, anyone else's table,
  not a number), for the line under each curve, for the store's rule,
  and for a run over a fit-1 table from 40 frames that refits it from
  22 and declares the current fit.

The number tracks the fit, not the develop. A change to the develop
that moves what the match sees leaves tables stale under the same
number; `display_curve` covers the one such change so far.

**Left.** The Python reference, `tools/camera-match/fit.py`, still
fits its residual in encoded sRGB with no such term. The sheet does
not yet check a stale group by itself; the Refit button opens it on
the one look. A stale table fitted on its own body, in a scope that
now has too few of that body's frames to fit, is not refitted: the
run falls back to a borrow, which still never replaces a body's own
fit, so the line stays. The `refit` example declares the fit too,
unless told otherwise.
