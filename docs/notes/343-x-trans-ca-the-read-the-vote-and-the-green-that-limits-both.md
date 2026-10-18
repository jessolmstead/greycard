# §343. X-Trans CA: the read, the vote, and the green that limits both (2026-10-17)

§331 left the X-Trans CA correction short of Bayer's and named two
causes: the resample's tent read of the color difference, and the
stand-in mosaic's vote, which reads 0.77 of a true Bayer one. We built
both halves and measured them apart and together. The read is better
and lands. Neither way of putting the vote right does what it should
on the bench, and the reason is a third thing §331 did not name: the
green the resample reads. That is where the shortfall is.

## The numbers we started from

All on the bench with 1.5 px of lateral CA at the corners (red out,
blue in by three quarters of it), Markesteijn's three passes, CPSNR in
sRGB, reproduced to the hundredth from master before anything changed:

| set | X-Trans: clean / aberrated / corrected | Bayer AMaZE: clean / aberrated / corrected |
|---|---|---|
| Kodak | 36.20 / 32.14 / 32.65 | 39.22 / 32.89 / 33.99 |
| McMaster | 34.65 / 31.08 / 31.63 | 35.96 / 31.58 / 32.50 |

§331's mosaic figures are, we found, these: the error left is the
mean over the 42 images of each image's RMS error after the
correction over its RMS error before it, at the red (blue) sites, 32
pixels in from every edge; the unaberrated RMS is the same mean of
each image's RMS change when the correction runs on the clean
mosaic. X-Trans leaves 86.1% / 99.4% (Bayer 80.1% / 90.0%), and moves
an unaberrated mosaic by 0.0083 / 0.0086 (Bayer 0.0073 / 0.0071). The
0.77 is the slope, through the origin over every tile and both
directions, of the stand-in's first-pass fit against the fit a true
RGGB mosaic of the same aberrated scene gives: median over the images
0.767 for red and 0.801 for blue.

## The read

The tent read a plane of color difference at the tent's centroid,
which on X-Trans is off the point. The read now takes the color's
sites in the open square of side five round the point and weights
them exact on a plane, with the least sum of w²/k for a kernel k: the
tent over the square divided by the squared distance to the point.
The pole makes the read the site's own difference when the point is on
it, as Bayer's bilinear read is, so a fit that finds a shift of nothing
moves nothing; the tent keeps it local. `sites.rs` has the weights
(`planar_weights_kernel`). The kernel's pole makes the system's
determinant tiny beside its scale while leaving it well determined, so
it is solved from the heaviest site by a Cholesky factorization whose
pivots are judged against their own diagonals. That test catches exact
degeneracy only (sites in a line, to rounding); a system merely near it
passes, with weights as large as it makes them, which on X-Trans never
happens (below). A first build solved it
by the determinant test `planar_weights` uses, which refused every
point within about a third of a pixel of a site for an inverse sixth
power and so corrected nothing there. That dead zone scored well
(32.78 / 31.70, and an unaberrated RMS of 0.0038 / 0.0026, since it
left alone every tile whose shift was under a third of a pixel), which
says the resample's own error at a small shift is larger than the
shift's; we did not keep a dead zone made by a determinant test.

Every read holds four to six sites; a test reads a flat field and a
ramp back at every site of the period at all 36 phases over shifts out
to the correction's limit of 3.99 px either way, and finds both exact.
A review's sweep of 839,056 reads found the least weight -0.0205, the
largest 1.0023 and the worst plane error 2.4e-8; the guard's
green-weighted mean takes the weights' positive part. Each tile's
shift is one for all its sites, so a tile builds its sixteen reads
once, which made the pass faster.

**By the frame's edge.** The tent's read held the point inside the
frame and dropped the sites past it. A first build of this read
dropped them and scaled the rest to one, which is no longer exact on a
plane, and the red error in the outer three rows and columns of the
synthetic radial test rose from 0.00304 / 0.00290 / 0.00324 (its three
phases) to 0.00340 / 0.00306 / 0.00327. Weighing the same sites anew,
exact on a plane at the point, did not recover it (0.00343 / 0.00308 /
0.00329). What does is a fresh read for such a site: the color's sites
inside the frame round the point held inside it, as the tent held it,
weighed exact on a plane (0.00312 / 0.00298 / 0.00327). Where those
sites do not span a plane, in the corners (144 of 1,280 such reads in
the test), the old read's sites inside are scaled to one. That costs
the bench 0.03 dB on Kodak, whose images are 768 wide, against the
first build:

| | Kodak | McMaster | error left R / B | unaberrated R / B |
|---|---|---|---|---|
| master (tent) | 32.65 | 31.63 | 86.1% / 99.4% | 0.0083 / 0.0086 |
| plane-exact read, edge scaled | 32.68 | 31.69 | 85.7% / 98.2% | 0.0079 / 0.0077 |
| plane-exact read, edge read anew (kept) | 32.65 | 31.69 | 85.9% / 98.3% | 0.0077 / 0.0079 |
| Bayer | 33.99 | 32.50 | 80.1% / 90.0% | 0.0073 / 0.0071 |

The read itself gains 0.03 / 0.06 dB and the edge gives the 0.03 on
Kodak back; we kept the edge exact rather than the bench's hundredths.

The kernel's power and the square's side hardly matter: the inverse
square and the inverse fourth power over squares of five and six are
within 0.03 dB of each other. The unaberrated RMS comes most of the way
to Bayer's and not all of it, because what is left of it is the size of
the spurious shift the vote finds on a clean mosaic times the
picture's gradient, not the read.

## The vote

The numbers of the vote's gain, the votes cast on the sites, the
read's kernel variants above, and the green and the true shift below
come from builds that were not committed: an experimental copy of the
correction with each choice behind a switch, on the read with its edge
scaled (the first build above).

**Weighted back.** The slope of the stand-in's fit against a true
RGGB fit, over the 42 images at each of the 36 phases (18 distinct:
a shift by (3, 3) gives the same numbers), runs from 0.73 to 0.79 in
red and 0.74 to 0.81 in blue, with no phase standing out: 0.761 and
0.778 pooled. A gain of 1.304 on red and 1.248 on blue brings the
fit to a Bayer one's. It makes the bench worse:

| | Kodak | McMaster | error left R / B | unaberrated R / B |
|---|---|---|---|---|
| tent, gain | 32.49 | 31.47 | 88.6% / 102.2% | 0.0098 / 0.0096 |
| plane-exact read, gain | 32.50 | 31.52 | 88.4% / 101.2% | 0.0093 / 0.0089 |

**Cast on the X-Trans sites.** RawTherapee's sums, each term read at
the X-Trans red and blue sites of the tile's interior rather than on
a Bayer lattice: the color difference and the green gradient from the
site, and the high- and low-pass filters the weight is made of from
planes holding each color at every site (the sample, or its
plane-exact estimate). The tiles, `vote_of` and `fit_votes` are the
reference's. Its fit reads 0.96 (red) and 0.94 (blue) of the true
RGGB one, and 0.82 / 0.82 of the true shift against the RGGB fit's
0.77 / 0.82: the under-read is gone.

| | Kodak | McMaster | error left R / B | unaberrated R / B |
|---|---|---|---|---|
| tent, cast on the sites | 32.56 | 31.68 | 86.1% / 99.7% | 0.0066 / 0.0068 |
| plane-exact read, cast on the sites | 32.57 | 31.74 | 85.8% / 98.8% | 0.0060 / 0.0060 |

Better on McMaster and on a clean mosaic, worse on Kodak.

**On the samples.** To see what is left of the aberration in a
developed frame without trusting the vote being judged, we mosaic the
CLI's preview as RGGB and fit it with the Bayer vote; the RMS of that
fit over the tiles is the aberration left (it reads about 0.25 on a
clean bench image, its floor there).

| frame, red / blue | uncorrected | master | read | read, gain | read, cast on the sites |
|---|---|---|---|---|---|
| X100VI | 1.060 / 0.644 | 0.118 / 0.107 | 0.120 / 0.120 | 0.091 / 0.097 | 0.170 / 0.140 |
| X-E4 | 0.189 / 0.311 | 0.038 / 0.063 | 0.040 / 0.050 | 0.034 / 0.060 | 0.047 / 0.062 |
| X-T5 | 0.115 / 0.092 | 0.049 / 0.038 | 0.050 / 0.043 | 0.046 / 0.038 | 0.073 / 0.057 |

The gain removes more of the aberration on all three frames, and on
the bench too (the same measure on the bench's output: 0.202 / 0.174
against 0.224 / 0.218), and still costs 0.18 dB there. The votes cast
on the sites leave more on all three: on the X100VI they read 2.03 px
of red where the stand-in reads 1.33, and 0.66 on the X-E4 where it
reads 0.89, a field of another shape rather than a larger one. At 1:1
the ridge and the leaves look the same under all of them.

Neither is kept. The gain measures better on the frames and worse on
the bench, the sites' votes worse on the frames, and the bench says
why: a fuller correction of a shift should not cost CPSNR, and it
does here because the resample loses more than it puts right.

## The green

The resample reads green at the point a site's light came from,
bilinear on a plane that holds each red and blue site's plane-exact
estimate from its five greens. Two experiments, not kept, say that
this is the shortfall:

- **The true shift.** Handing the resample the aberration's own field
  for one pass, in place of the fit, gives 32.79 / 32.00: 0.11 and 0.31
  dB over the fit. The vote costs at most that much.
- **A demosaic's green.** Handing it Markesteijn's three-pass green,
  made each pass from the mosaic as it stands, gives 33.58 / 32.12 with
  the stand-in's vote, 0.9 and 0.4 dB over the plane-exact green, and
  33.71 / 32.44 with the true shift, against Bayer's 33.99 / 32.50.
  With that green the votes cast on the sites give 33.42 / 32.18 and
  the gain 33.42 / 31.97.

So the green is worth four to eight times what the vote is, and the
best X-Trans could do with the true shift and a demosaic's green is
within 0.3 dB of Bayer. A three-pass Markesteijn a pass costs more than
the whole correction does (0.6 s on 40 MP at 32 threads against
0.65 s for the correction at four), so the experiment is a ceiling, not
a build.

## The frames and the time

The three samples measure the same shifts as before: the X100VI 1.33
px red and 1.48 blue from 3,143 tiles (3,145 before), the X-E4 0.89
and 0.97 from 2,127, the X-T5 0.21 and 0.38 from 3,273. Against no
correction the frame means (R / G / B, of 255) move by:

| frame | master | this build |
|---|---|---|
| X100VI | +0.0504 / +0.0240 / -0.0198 | +0.0521 / +0.0225 / -0.0165 |
| X-E4 | -0.0017 / +0.0133 / +0.0182 | +0.0020 / +0.0101 / +0.0172 |
| X-T5 | -0.0267 / +0.0192 / -0.0219 | -0.0175 / +0.0085 / -0.0069 |

The X100VI's red moved by just over 0.05 under master already and
moves by 0.0017 more here; the rest move by less. The aberration left
in the three frames, by the measure above, is 0.120 / 0.120, 0.040 /
0.050 and 0.050 / 0.043. The ridge against the sky and the leaves'
specks are as clean as master left them.

At four threads on the X100VI's 40 MP the correction takes 0.63 to
0.65 s (least of five, three rounds) against master's 0.75 to 0.80 s in
the same rounds, the machine at a load of eight (an earlier set at a
load of ten: 0.67 to 0.79 against 0.93 to 0.97); the reads are made
once a tile rather than looked up sixteen times a site, and an edge
site's fresh read costs nothing measurable. A Bayer frame is the
same to the bit: `ca.rs` is not touched, a test pins a synthetic Bayer
frame's corrected bits to master's digest, the 45 MP CR3 gives the same
bits (37c3cf6e2b9a173c) on both builds, and the bench's AMaZE rows are
the same to the hundredth.

The synthetic radial test (0.5% red, -0.4% blue, 400x320) leaves 27.2%
/ 28.4%, 27.5% / 28.7% and 27.6% / 28.4% of the error at its three
phases, against 27.1% / 28.3%, 27.4% / 28.5% and 27.5% / 28.2% before
and Bayer's 29.3% / 30.1%: its smooth texture never showed the
shortfall, and still does not.

## What is left

- The green the X-Trans resample reads (and the votes cast from it):
  the plane-exact five-green estimate is soft, and a demosaic's green
  is worth 0.4 to 0.9 dB on the bench, more than the read and the vote
  together. A green estimate that uses the red and blue round a site,
  as Markesteijn's first step does, at a cost the correction can carry.
- The vote, again, once the green is better: with a demosaic's green
  the votes cast on the sites were the best of the three on McMaster.
  The gain (1.304 / 1.248) and the sites' votes are measured above and
  can be rebuilt from them.
- A GPU version of the X-Trans correction, if the CPU's 0.65 s becomes
  what a develop waits on.
