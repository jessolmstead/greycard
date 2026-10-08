# 264. The GPU ops checked at random settings (2026-10-08)

§221 checks the viewport shader against the CPU at 400 random edits.
The three GPU op ports, the sharpen (§114), the CA correction and
Texture and Clarity (§259), were checked only at settings chosen by
hand. Each now has a seeded sweep in §221's manner. Building them
found a crash that shipped in every release and a place where the
viewport and the export disagreed, both in the CA correction.

### The sweeps

A splitmix64 generator seeded per (seed, index), in
`greycard-gpu/tests/common`, draws every option over the editor's
range, each end a fifth of the time; a signed slider also lands on
exactly zero a tenth of the time and on a small value (under a
twentieth of its range) three times in twenty. Sizes straddle each
op's internal steps (the sharpen's tile widths at 1984 and 3968,
Clarity's grid from a long edge of 2540, Texture's radius from 5000,
the CA's 256 minimum and its 112-pixel tile steps), and the scenes are
drawn from several kinds (soft, steps, noise, gratings, sparse, flat,
dark, saturated, clipped patches). `GREYCARD_OP_PARITY_SEED`, `_CASES`
and `_ONLY` replay a case; a failure prints the seed, the index, the
drawn settings and the adapter. The sharpen draws 30 cases, Texture
and Clarity 50, the CA 24; each sweep takes a few seconds in debug.
Options the editor doesn't expose (the sharpen's early stop off, the
CA's passes and guard) are drawn too, as extra coverage.

**Sharpen and local contrast** use their files' own comparisons. Two
tolerances changed, each for a stated reason. The sharpen mask is a
sigmoid sloped at eight over the threshold, so at small thresholds one
ulp of input moves the CPU's own mask past 1e-4; past the plain limit a
mask pixel must lie within the CPU's masks at the threshold moved by
its contrast's rounding, for at most 1% of pixels. A pixel that fifty
unstopped iterations darken to a thousandth of its input carries its
input's rounding, so with the early stop off it is measured against a
hundredth of its input. No GPU bug was found; hand mutations of each
shader (a constant 2% off, a window edge, a tap) were caught.

**The CA** took most of the work, because the correction is badly
conditioned exactly where it decides: a guard reading the sign of a
residue beside a clipped patch, a tile's stencil and direction
switching as its shift crosses an integer or zero, votes that are
quotients on flat tiles, a least-squares fit whose decisions flip.
One ulp of input moves the CPU's own output by up to 169,000 samples.
Lavapipe, which rounds as the CPU does, matches it to the bit; NVIDIA
and RADV, which fuse multiply-adds, land inside that spread. A
tolerance set from the CPU's spread under nudged input hid real
shader bugs, so the sweep takes the correction apart where the GPU's
work is:

- **Votes, tile by tile**, against the CPU's on the same input. Each
  vote's class is decided from the CPU alone, so no GPU bug can move a
  vote into a looser class: six graded nudges of the input (1, 4 and
  16 ulps each way), and two edge probes that move every green
  interpolated past the picture's edge by an ulp each way, since a
  mirrored read is the same sample and no input nudge breaks its tie
  (`measure_coefficients_edged`; production passes no nudge). A vote
  the CPU itself moves by 1e-3 px an ulp or more, one cast in some
  runs and not others, or one whose energy the nudges move more than
  twofold, is counted and capped (55% of a pass, 5% of a run) but not
  held to a value. Every other vote must be cast on both paths, its
  shift within 4.3e-3 px plus 1.1e-3 of itself and its weight within
  1.6% of the CPU's. The edge probes class about 0.12% of edge-tile
  votes and none inside.
- **Each pass** against the CPU's resample run on the fit from the
  GPU's own votes (`resample_with`, the pass's own stage): a sample
  within 1e-4 relative, else within its rounding reach (from its gain,
  capped at 6e-3 of its scale, for at most 0.012% of a run's samples),
  else only where a guard sits within reach of flipping and the GPU
  took the flipped value (3% of a pass, 0.06% of a run).
- **The run** equal to its passes chained, and the color-shift guard
  held strictly against `avoid_color_shift`.
- **The fit is not bounded.** It is the CPU's `fit_votes` on both
  paths, and the GPU's only inputs to it, the votes' shifts and
  weights, are checked above. On synthetic scenes the CPU's own fit
  is chaotic: one vote moved by a millionth of itself moved it 6.2 px.
  A backstop caps the share of a run's passes that fit more than
  1e-3 px off at 28%; unmutated runs reach 17%, a weight bug 52 to 100%.
  The end-to-end check of the fit is an ignored test on real frames
  (`GREYCARD_SAMPLES`): every Bayer raw, GPU against CPU, the fit
  within 1.2e-5 px and at most 0.012% of samples off. On the 31 test
  raws the fits agree within 5.5e-6 px.

The caps were set from seeds 1 to 200 and four others, on NVIDIA and
RADV, each with about twice the worst seen and the measurement beside
its constant, then held out: seeds 201 to 320 failed the earlier
designs (which is how the fit bound and a vote floor at the edges
went), and 321 to 470 pass the final one untouched, as do a reviewer's
fresh seeds. Of 26 hand mutations of the CA shader, the sweep catches
25; the vote kernel's gradient epsilon, which only matters on
near-black tiles, is caught by the fixed tests alone. The fixed tests
keep master's strict limits, with RADV allowed one sample on one pass.

### What it found

**A crash in the CPU's CA correction**, in every release from 0.1.0. A
mosaic 105 to 111 pixels past a multiple of the 112-pixel tile
interior has a last column of tiles whose interior lies wholly past
the edge, and writing its empty rows indexed off the end of the
mosaic on the last row. Such tiles are now skipped; the interiors
before them cover every column, as in RawTherapee, whose write loop
writes nothing there either. It struck wherever the CPU corrects: the
command line, the camera match, every export (an export always runs
the CA on the CPU), and the editor without a working GPU. None of the
31 test raws has such a width.

**The viewport and the export disagreed beside clipped highlights.** On
an exactly flat patch the CPU's interpolated green gives exactly zero
gradient and the tile casts no vote; the GPU's fused arithmetic left a
rounding residue and cast votes, up to hundreds of pixels, which moved
its fit. On 5 of the 31 raws the export (CPU) and the viewport (GPU)
then differed by up to 37 levels on 0.1 to 0.4% of pixels. The green at
a red or blue site is now evaluated as its upper neighbor plus the
weighted differences from it, on both paths: the same mean as
RawTherapee's weighted sum, `wu·u + wd·d + wl·l + wr·r = W·u +
wd·(d−u) + wl·(l−u) + wr·(r−u)`, but four equal greens now give that
green exactly, and near-equal ones round on their differences. Every
frame's CA output then agrees between the paths (fits within 6e-6 px,
no vote on one side only), and the viewport-to-export difference
falls to at most 10 levels with the 99.9th percentile at 0. The user
chose this knowing it changes the CPU's arithmetic everywhere: against
0.4.0, an export moves on 0.004 to 0.06% of its pixels, single pixels,
the 99.9th percentile 0 (1 on two frames), the worst one pixel 62
levels on a branch against a blown sky. The module's departures
paragraph records the change of order from RawTherapee. Speed is
unchanged.

**What remains of the gap** comes after the CA: the demosaic's
direction choices (AMaZE, RCD and VNG4 alike) flip on inputs that
differ by 1e-5, turning a difference of a millionth in the mosaic into
a single pixel a few dozen levels off, and the capture sharpen
roughly doubles it. On the roadmap.
