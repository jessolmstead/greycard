# 281. How many frames the camera match develops (2026-10-10)

The camera match (§180, §181) develops about 40 frames a group, spread
across folders and dates, and that number was never swept: the trial's
sets were 22 to 32 frames. The roadmap's open question asked what the
held-out error does at 10, 20, 40 and 80 frames, picked at random and
picked for the coverage of the table's lattice, on the trial's four
archive sets.

## Method

**The match as the editor runs it.** The sweep is an ignored test in
the editor (`camera_match::sweep`), so a frame is measured by the
runner's own `measure`: the export's develop at a 2048 long edge under
the default display curve, the camera's JPEG laid over it, the block
pairs, the exposure iterated onto the camera's; and a group is fitted
by `fit_group` with the default `LutParams`. Each frame's pairs are
kept on disk with the seconds they took, so the develop runs once and
every pick is a fit over records.

**The sets.** The trial's four, by body and style as the sheet groups
them, with the frames the archive has gained since the trial: R6 II
Faithful (628 fixed frames), R5 II Standard (547), R5 II Faithful (22,
so its sizes stop at 14) and GFX 100S II Reala Ace (517). A fifth set
is the GFX with the Sigma frames taken out, for the question §180 left
open.

**A fixed pool held out of every fit.** Per set, 12 frames (16 on the
GFX, 8 on the R5 II Faithful) drawn once at random and left out of
every pick, so every size and every pick is measured on the same
frames: the model fitted on the pick, applied to each pool frame's
pairs, the mean over the pool of each frame's mean ΔE. Beside it, the
sheet's own figure, every fitted frame held out of its own fit, and
the four-frame figure §181 first reported.

**The picks.** Three random draws (nested: the 20 hold the 10), the
sampler as it ships (`sample`), and two coverage picks.

**The coverage pick.** Each candidate's camera JPEG is box-averaged to
about the develop's size and cut into the fit's 24-pixel blocks; the
flat, unclipped ones, by the pairs' own cuts on the camera's side,
are counted into the table's lattice coarsened four to one (8³ cells
of the encoded domain). A cell counts as filled at 64 blocks, about
one a node of the 4³ it spans, or at every block the candidates have
of it. The greedy step takes the frame that adds the most weighted
filling; the weight is the square root of how often the candidates
show the cell ("asked about"), or one for every cell ("even"), a cell
only one frame shows counting for nothing. A frame with over 5% of its
pixels clipped waits until no other is left; every lens with three or
more candidates gets a frame, forced in when the picks left are as
few as the lenses owed; ties go to the folder picked least.

The frames are the ones the sampler takes from: the fixed frames, since
every set has twenty of them. 1,092 frames were developed in all, two
of them dropped by the registration cutoff, 7.1 seconds a frame on
average under `nice` with the desktop in use. The tables below are
ΔE in Oklab; 0.02 is about a just-visible step. The random rows are
the mean of three draws, with the range on the pool.

## Results

Held out on the pool, by size and pick:

| set | pick | 10 | 20 | 40 | 80 |
|---|---|---|---|---|---|
| R6 II Faithful | random | 0.0116 (0.0112-0.0122) | 0.0110 (0.0106-0.0114) | 0.0108 (0.0104-0.0110) | 0.0107 (0.0106-0.0108) |
| | sampler | 0.0112 | 0.0111 | 0.0108 | 0.0108 |
| | coverage, asked about | 0.0113 | 0.0114 | 0.0112 | 0.0111 |
| | coverage, even | 0.0116 | 0.0111 | 0.0112 | 0.0110 |
| R5 II Standard | random | 0.0154 (0.0137-0.0167) | 0.0138 (0.0129-0.0148) | 0.0132 (0.0125-0.0138) | 0.0124 (0.0121-0.0127) |
| | sampler | 0.0139 | 0.0139 | 0.0125 | 0.0127 |
| | coverage, asked about | 0.0140 | 0.0132 | 0.0140 | 0.0149 |
| | coverage, even | 0.0140 | 0.0146 | 0.0150 | 0.0149 |
| GFX Reala Ace | random | 0.0125 (0.0122-0.0127) | 0.0119 (0.0118-0.0120) | 0.0111 (0.0109-0.0112) | 0.0110 (0.0109-0.0110) |
| | sampler | 0.0134 | 0.0114 | 0.0110 | 0.0109 |
| | coverage, asked about | 0.0131 | 0.0124 | 0.0116 | 0.0117 |
| | coverage, even | 0.0149 | 0.0132 | 0.0123 | 0.0118 |
| GFX, GF lens only | random | 0.0102 (0.0099-0.0105) | 0.0097 (0.0096-0.0098) | 0.0095 (0.0093-0.0097) | 0.0092 (0.0091-0.0093) |
| | sampler | 0.0106 | 0.0097 | 0.0094 | 0.0091 |
| | coverage, asked about | 0.0107 | 0.0108 | 0.0098 | 0.0096 |
| | coverage, even | 0.0121 | 0.0122 | 0.0101 | 0.0096 |

R5 II Faithful has 22 fixed frames, so 14 after its pool: at 10 the
random draws and the sampler hold out at 0.0148 and the coverage picks
at 0.0223; at 14 every pick is the same frames and holds out at 0.0148.

The sheet's figure (every fitted frame held out of its own fit) and the
earlier four-frame one, with the seconds the match would spend, at 20
and 40 frames:

| set | pick | every frame, 20 | every frame, 40 | four, 40 (range of the draws) | seconds, 40 |
|---|---|---|---|---|---|
| R6 II Faithful | random | 0.0125 | 0.0119 | 0.0133 (0.0105-0.0166) | 173 |
| | sampler | 0.0118 | 0.0117 | 0.0148 | 173 |
| | coverage, asked about | 0.0133 | 0.0137 | 0.0163 | 167 |
| R5 II Standard | random | 0.0152 | 0.0152 | 0.0134 (0.0077-0.0170) | 252 |
| | sampler | 0.0112 | 0.0128 | 0.0088 | 257 |
| | coverage, asked about | 0.0180 | 0.0156 | 0.0148 | 271 |
| GFX Reala Ace | random | 0.0119 | 0.0123 | 0.0120 (0.0106-0.0142) | 421 |
| | sampler | 0.0123 | 0.0121 | 0.0108 | 399 |
| | coverage, asked about | 0.0148 | 0.0134 | 0.0144 | 413 |

Eighty frames take twice the seconds of forty: 352 on the R6 II, 499
on the R5 II, 865 on the GFX. The fit itself, with every frame held
out, is 2 to 5 seconds at forty and 15 to 27 at eighty; the rest is
the develop.

How much of the lattice each pick reaches, on the asked-about weights
(the coverage pick's own score), at 20 and 40: the coverage pick
0.94 to 0.96 and 0.97 to 0.99, random 0.78 to 0.90 and 0.87 to 0.95,
the sampler 0.79 to 0.89 and 0.90 to 0.93. On even weights the gap is
wider: coverage at 10 reaches more of the lattice than random at 80 on
every set. The coverage pick's gain per frame falls under 1% at 9 to
12 frames on the asked-about weights and under 0.25% at 19 to 24; on
even weights under 1% at 18 to 20 and under 0.25% at 46 to 62.

The GFX pool by lens, random picks: the GF 110mm's frames hold out at
0.0106, 0.0101, 0.0097 and 0.0095 at 10 to 80 frames, the Sigma 50mm's
at 0.0168, 0.0159, 0.0142 and 0.0141.

## Reading

**Forty is the right number.** Held out on a fixed pool, the error
falls from 10 to 40 frames and is flat after: from 40 to 80 the
random draws gain 0.0001 on the R6 II and the GFX, 0.0003 on the GF
frames alone, and 0.0008 on the R5 II Standard, which is the set with
the most lenses and places and the one still learning; the sampler
gains nothing on any set from 40 to 80 (0.0108 to 0.0108, 0.0125 to
0.0127, 0.0110 to 0.0109, 0.0094 to 0.0091). Those are a twentieth of
a visible step for twice the minutes. Twenty is cheaper and costs
0.0003 to 0.0014 against forty with the sampler, the most on the R5 II
Standard; the draws' own spread at forty is 0.0003 to 0.0013, so a
difference under about half a thousandth is not a finding. The cap
stays at 40.

**The sampler is already worth twice its frames.** Spread by folder
and date, 20 frames hold out within 0.0003 to 0.0006 of 40 random
ones, and 40 within 0.0001 to 0.0002 of 80 random ones, on the three
sets with more than forty frames and on the GF frames alone. A library's frames come in sessions, and a session's frames are
near-copies of one light and one lens; taking a few from every folder
first is what spends a frame on something new. Random draws, which
land where the sessions are long, need twice as many.

**Coverage picking loses.** It does what it was built to: at 20 frames
it reaches more of the lattice than random does at 80. And it holds
out worse: at 20 it is within 0.001 of random at 40 on the R6 II
(+0.0007) and the R5 II Standard (0.0000), 0.0013 off on the GFX and
on the GF frames alone, and on the R5 II Faithful at 10 it holds out
at 0.0223 against 0.0148 for every other pick. At the same size it is
worse than random on four sets of five (the R5 II Standard at 20 is
the exception), and at 40 and 80 worse than the sampler on every
set. The even weights, which chase the rare cells harder,
are worse again. Two things say why. The error left after the look is
lightness, not color: §180 put the chroma error under 0.002 on every
set, and lightness varies by scene, by light and by lens, which the
JPEG's colors do not show. And the frames that fill rare cells are
the unusual ones, saturated subjects and odd light, which a pool of
ordinary frames does not ask about and which, held out of their own
fit, are the hardest to predict: the coverage picks' every-frame figure
is 0.001 to 0.007 above the sampler's. Filling the lattice's corners
helps a picture of a saturated subject the library rarely holds, which
a pool drawn from the library is not; that is a case for a shoot of
colorful subjects or a chart (`docs/camera-match.md`), not for a
different sampler. The part B condition (coverage at 20 within about
0.001 of random at 40 on three sets of four) is met on two, so the
sampler is not changed.

**The GFX's gap is the Sigma, not the count.** The Sigma 50mm's pool
frames hold out at 0.0142 at 40 frames and 0.0141 at 80, flat, while
the GF 110mm's hold out at 0.0097 and 0.0095; and a fit with the
Sigma's frames taken out holds the GF frames at 0.0095 and 0.0092,
only 0.0002 to 0.0003 better than the fit that has them. More frames
do not close it, and having the lens in the fit costs the other lens's
frames next to nothing. That fits §180's reading, the lens's own
falloff, which no profile corrects and the camera's JPEG does not
carry (the sweep did not split the error by radius to check it again),
and it belongs to the lens track's vignetting line. The GFX with the
GF lens alone is the best-fitting set of all, 0.0092 held out at 80.

**The sheet's figure runs above the pool's and the four-frame one was
noise.** Every frame held out of its own fit lands 0.001 to 0.002 over
the pool at forty, more for picks of unusual frames, since it measures
each frame against a fit that lacked it; it is the honest figure and
the one the sheet shows. The four-frame figure §181 first reported
ranged from 0.0077 to 0.0170 across three random draws of forty on
the R5 II Standard; it was right to go.

## What changed

Nothing in the runner. The sweep is in the tree to be run again,
`camera_match_sample_size_sweep` in the editor's tests (ignored; its
module says what it reads and writes), with the coverage pick it
measured (`camera_match::cover`, test-only, with its own tests) and
`tools/camera-match/sweep_tables.py` for the tables. A future cap
change or sampler can be measured against the same pools by adding a
pick to the sweep.

Open: whether the R5 II Standard, whose random draws still gain at
80 where its sampler does not, says that a
group with many lenses and places wants more than forty, or a cap
that scales with the number of folders; whether skipping clipped
frames (5% of pixels, which passed over a quarter to two fifths of the
candidates) cost the coverage pick anything; and a pool drawn from
saturated subjects, which is where a coverage pick could still earn
its keep.
