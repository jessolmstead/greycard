# §331. X-Trans, step 2: noise, hot pixels and chromatic aberration (2026-10-16)

Step 1 (§322) developed X-Trans raws and stepped around every stage
before the demosaic that read a 2x2 Bayer mosaic: the noise
measurement, and with it the hot pixel repair and the profiled
denoise, and the chromatic aberration correction. This step gives the
three of them an X-Trans version. What is left on X-Trans is the
capture sharpen's radius measurement and the learned denoiser.

## Sites that sit at no one offset

Every one of the three read its neighbors at fixed offsets: the four
same-color sites two photosites away for the noise residual, the eight
for the hot pixel test, the 2x2 lattices of red and blue for the CA
vote. On X-Trans the nearest red to a red is two columns over at one
site and a row and two columns over at the next, and the set round a
site is lopsided: the five reds in a red site's 5x5 have their
centroid off the site. A plain mean of them reads the picture a
fraction of a pixel away. On a gradient that is a bias, and on an edge
it is a shift.

`develop/sites.rs` builds what each stage reads once a call, a cell
per site of the pattern's period (`Phases`). `planar_weights` weights a
set of sites so that their weighted sum is exact on any plane (a
constant and both gradients) with the least sum of squared weights,
which is the least noise such a mean can carry: the minimum-norm
solution of three equations, in double precision. On X-Trans every
weight it gives is positive. `interpolation` estimates any color at any
site from the color's sites in the 3x3 where they span a plane, else
the 5x5: green at a red or blue site from its five greens, red at blue
and blue at red from three, red and blue at a green site from the
5x5. A test checks every estimate on a plane at all 36 phases and on
Bayer. None of this is a port.

## The noise measurement

The estimator (ours, §13) takes, per 16x16 block and color, the
residual of each sample against its same-color neighbors. On X-Trans
the residual is against every same-color site in the 5x5, weighted by
`planar_weights`, and each squared residual is divided by its own
`1 + sum w^2`: 1.2 at a red or blue site (five neighbors), 1.06 to
1.08 at a green one (twelve to sixteen, the adjacent greens of a 2x2
block among them). A clean ramp is silent at every phase, which a
plain mean of the lopsided neighbors would not give. The Bayer path is
unchanged, to the bit.

Two questions came up and were settled on the samples. Greens in a 2x2
block and solitary greens might read differently (as Bayer's two
greens can), which would put a fixed-pattern difference into the
residual. In the X-E4 frame's quietest bin the four classes (red,
blue, block greens, solitary greens) give the same variance over level
to within 12% (5.2e-5 to 5.8e-5), and leaving the adjacent greens out
of the residual only raised the green estimate. And red and blue sites
are two in nine against one in four on Bayer, so the line's intercept
scatters more: a test on a known model across four phases finds the
estimate within 6% from a level of 0.1 up and within 20% at 0.02.

On the X100VI (ISO 500) and the X-T5 (ISO 3200) the three channels
agree at mid grey, as a sensor's do (0.0040, 0.0042, 0.0039; 0.0098,
0.0096, 0.0097). On the X-E4 (ISO 640) they do not: 0.0100, 0.0172,
0.0170. That frame is foliage edge to edge with no flat area above the
shadows, and the line through the bins that texture lifts is what
reads high. It is the estimator's known limit (§13, "a textured frame
with no flat area at some level overstates the noise there") rather
than the X-Trans residual: the frame's quiet bins agree. On the bench,
noise of 0.02 at mid grey on Kodak and McMaster is estimated at 1.61
to 1.74 times the truth on X-Trans against 1.52 to 1.62 on Bayer, the
same texture's doing.

## The hot pixel repair

darktable's `hotpixels.c`, which the repair follows, has an X-Trans
path: for each site of the 6x6 a table of the four radially nearest
same-color sites, found by a search outward. Ours builds its tables
from the pattern the same way, with what the Bayer rule reads carried
over. The same-color test reads every site of a green candidate's
color in its 5x5 (twelve to sixteen, the adjacent greens of a 2x2
block among them) and of a red or blue candidate's in its 7x7
(eleven). The others test reads each of the two other colors: its
sites in the 3x3 as the adjacent ones, its sites two and three out as
the background. Every 3x3 of X-Trans holds all three colors, so each
other color has an adjacent site at every candidate.
`Candidate::others` is now three options, the Bayer positions or the
two X-Trans colors.

**Why red and blue read the 7x7.** A first build read the 5x5 for
every color. A red or blue site has five same-color sites there to
Bayer's eight, and a speck of texture clears five far more often. On
the texture test's 2x2 cells, over 100 seeds at three phases, the
repair took 1.95 sites a frame against Bayer's 0.14, and more than one
in half the frames. Over the 7x7 it takes 0.02 (six sites in 300
frames). The same-color test alone takes 369 sites of one such frame,
where over the 5x5 it took 1,163. The test now runs ten seeds at three
phases of each mosaic and asks that X-Trans take no more than Bayer:
2 sites against 13. Green keeps its 5x5, whose twelve to sixteen sites
already outnumber Bayer's eight.

A test runs every site of the period at three phases through the
Bayer test's cases. A lone hot or dead site goes and nothing else
changes. Any adjacent site lit two and a half times (or dark) spares
it, a green of the same 2x2 block as much as a site of another color.
Another color's site two or three out lit to twice its level does not
spare it. That is twice, not the Bayer test's two and a half: the ring
was set when a red or blue site read the five same-color sites of its
5x5, where a noisy site lit by two and a half could stand three times
above all five and become a candidate of its own; the 7x7's eleven
leave it at two, which the texture test holds.

On the samples, at the defaults: 3 sites on the X-E4 (of 802
candidates, the foliage's specks), 9 on the X100VI (of 14) and none on
the X-T5. Among the X100VI's are a green site 22 times its neighbors
and a red one 14 times; both were colored dots, and both are gone. The
7x7 kept the X100VI's nine. It let go of a blue site on the X-E4 that
stood 3.2 times above its five and of the X-T5's two red ones,
3.4 and 3.6 times above theirs at ISO 3200; at 1:1 the X-T5's two do
not stand out of the noise.

The tables made the Bayer path's reads a loop over a slice, and that
cost: 0.146 s against 0.090 on a 45 MP CR3 at four threads, same
bits. The same-color offsets are now a fixed length per mosaic (eight,
or sixteen padded with a repeat, which the largest and smallest of
them do not see) and the row's table cycles rather than take a
remainder per site. It is 0.065 s now, faster than before, and still
the same bits. X-Trans takes 0.13 s on 40 MP, sixteen reads a site.

## Chromatic aberration

RawTherapee's automatic correction, which `develop/ca.rs` ports, does
not run on X-Trans. `RawImageSource::preprocess` calls `CA_correct_RT`
for a Bayer sensor only, under the comment "Auto CA correction
disabled for X-Trans, for now...", and RawPedia sends an X-Trans user
to the manual correction after the demosaic. The engine has that as
the lens correction's red and blue scale. darktable's port of the same
code is Bayer only as well. Its X-Trans answer is another algorithm,
"chromatic aberrations" (`cacorrectrgb.c`), which runs on the
demosaiced picture and pulls red and blue toward green's local detail
through two guided manifolds. It treats axial aberration and blur as
much as a shift, moves color everywhere a little, and darktable leaves
it off by default. Our lateral correction is on by default on Bayer.
The same correction on both mosaics is one a user can reason about, so
we carried RawTherapee's to X-Trans (`develop/ca_xtrans.rs`) rather
than port darktable's.

What is RawTherapee's, through `ca.rs` unchanged: the tiles, each
tile's vote for a shift of red and of blue against green, the median
and the gate, the polynomial fit, the iterations, the resample's
guards and the color-shift avoidance's factor and blur. How X-Trans
reaches them is ours:

- **The votes** are cast by `measure_votes` on a stand-in Bayer
  mosaic: at every site the color RGGB puts there, the X-Trans sample
  where the colors match and the plane-exact estimate where they do
  not. A plain mean would not do here. The stand-in samples the 6x6
  period at only some of its phases, so the plain mean's offsets would
  not cancel over a tile and would be measured as aberration. A test
  checks the stand-in of a plane is the plane.
- **The resample** works on the X-Trans sites. At each red and blue
  site it takes green at the point the fitted shift says the light
  came from and the color difference there, once a pass. It reads
  those differences back at each site with a tent of radius two over
  the site's color round the point, where on Bayer they sit on a
  lattice. The guard's weighted mean is the reference's weights times
  the tent. The point is the site moved back by the whole shift; the
  reference moves it by the fractional part, which is the same below a
  pixel. Each site's difference is taken by its own tile's shift,
  where the reference takes a site near a seam by the shift of the
  tile being corrected.
- **The color-shift avoidance** gathers its factors on 3x3 cells, the
  counterpart of the reference's 2x2, with the blur's sigma two thirds
  of the reference's so it spans the same photosites.

On a synthetic radial aberration (0.5% red, -0.4% blue, 400x320, a
smooth texture) the X-Trans fit's largest shifts are within 0.05 px of
the Bayer fit's at three phases, and the correction leaves 27% of the
red error and 28% of the blue, against Bayer's 29% and 30%. On real
pictures it does less. On the bench, with 1.5 px at the corners:

| set | X-Trans: clean / aberrated / corrected | Bayer AMaZE: clean / aberrated / corrected |
|---|---|---|
| Kodak | 36.20 / 32.14 / 32.65 | 39.22 / 32.89 / 33.99 |
| McMaster | 34.65 / 31.08 / 31.63 | 35.96 / 31.58 / 32.50 |

CPSNR in sRGB. X-Trans gets back half a decibel where Bayer gets back
one, and the gap is in the correction, not after it. On the mosaic,
over the 42 images, the X-Trans correction leaves 86% of the red error
and 99% of the blue, against Bayer's 80% and 90%. Both halves of it
contribute:

- The first pass's fit from the stand-in mosaic reads 0.77 of the fit
  a true RGGB mosaic of the same scene gives. The estimates that make
  the stand-in are interpolations, which soften red and blue against
  green and so shrink the vote. The shifts reported for the samples
  below are under-read by about that.
- The resample's tent of radius two blurs red and blue more than the
  Bayer lattice's bilinear read does. On unaberrated images it leaves
  an RMS error of 0.0083 in red and 0.0086 in blue, against Bayer's
  0.0073 and 0.0071.

The follow-up is plane-exact weights (`sites.rs`) for the color
difference's read in place of the tent, and a vote that accounts for
the stand-in's softening.

On the samples: the X-E4 measures 0.89 px red and 0.97 blue from 2,127
tiles, the X100VI 1.33 and 1.48 from 3,145, and the X-T5 0.21 and 0.38
from 3,273. The red and cyan fringes on the X100VI's ridge against the
sky and on the X-E4's specks of light through leaves are gone. The
frame means move by under 0.05 of 255.

The GPU's correction is Bayer only. An X-Trans frame is handed to the
corrector like any other; the GPU declines it, and the worker runs the
CPU reference, in the viewport and the export alike. At four threads on
the X100VI's 40 MP that is 0.84 s, against 0.73 s for the CPU
correction of a 45 MP CR3. A first build took 1.22 s: a color lookup
whose period the compiler did not know, and green at the shifted point
made three or four times a site. The reduced develop prepares on the
full mosaic, so it pays the correction too.

## The denoise

The profiled denoise runs after the demosaic on interleaved RGB and
needs from the mosaic only the noise model, so with the measurement it
runs on X-Trans unchanged. A test develops a noisy synthetic X-Trans
frame and finds the model within 20% of the noise put on and the spread
halved. On the bench at noise 0.02 it adds 3.15 dB on Kodak and 2.85 on
McMaster, against 3.23 and 2.80 on Bayer. On the X-T5 at ISO 3200 the
CLI's develop takes 12 s with it against 3.9 s without, at four
threads, and the worm-like chroma of Markesteijn's demosaic on noise
goes.

The learned denoiser is not touched. Its contract is a packed,
stabilized 2x2 Bayer mosaic, and an X-Trans frame is still refused
before anything is prepared or loaded. That is the fallback step 1 put
in, now for the learned denoiser alone: the develop runs without it
and says so ("the learned denoiser has no X-Trans version yet"). The
NOISE section's hint is now "learned: not on X-Trans yet", and only the
tiers rest. A tier already set on an X-Trans frame (by a sync from a
Bayer one) can still be turned off: the control then offers off and
that tier, and no other. The profiled **Denoise** works as
on any raw. The bench still refuses `--ai-model` with `--pattern
xtrans`, and takes `--aberrate` and `--denoise` there now.

## What is left

- The capture sharpen's radius measurement on X-Trans. An automatic
  sharpen takes its default radius, and the log says so.
- A learned denoiser for X-Trans: a packing of its own and training
  data.
- A GPU version of the X-Trans CA correction, if the CPU's 0.84 s
  becomes what a develop waits on (the reduced develop's especially).
- The X-Trans CA correction's shortfall against Bayer's (86% / 99% of
  the error left, against 80% / 90%): plane-exact weights for the
  color difference's read in place of the tent, and the stand-in's
  vote read at 0.77 of a true Bayer one.
- The noise estimate on a frame of texture with no flat area, on any
  mosaic (the X-E4's).
