# 164. The wavelet chain, profiled and rebuilt (2026-09-24)

Roadmap v0.3.0's wavelet line, built overnight in wave B on Fable 5.1
and read by an opus reviewer whose retake on the quiet machine is the
headline.

§128 left the profiled denoiser's à trous chain as the next thing to
take on, and read its scaling — 7.25 s to 12.5 s from sixteen cores to
six, 1.7x for 2.7x fewer — as the base develop's kind, memory-bound,
where more cores would not help. That reading was wrong. The chain was
compute-bound on three things at once, none of them memory, and the
proof is below in the subnormals: two frames of the same size and the
same traffic ran the same level at twice each other's speed. The
figure a tester can watch, the whole 45 MP develop with the hybrid
denoise at six threads, goes from about 26 s to 12.7 s at the median
and 23.6 to 12.7 at the minimum, 2.0x, on a reviewer's quiet machine
with the binaries alternated five times each; 1.5x at the full count.

**How it was measured.** A harness under `target/work` decodes and
demosaics a frame once, keeps the RGB and the noise model on disk, and
times `denoise_profiled` on it alone, method by method, with the
denoiser's own phase timings now at `log::debug!` (`-vv` on the CLI
shows them). The frames are §128's: the 8192x5464 R5 Mark II
(`4Z4A2764.CR3`, 45 MP) and a 6000x4000 R6 Mark II (`5M0A8354.CR3`,
24 MP). Six threads is `taskset -c 0-5` with `RAYON_NUM_THREADS=6`, as
§128 did; the full count is 32. The machine was shared with other
builds all night, so the load average sits beside every number; the
before and the after ran alternately within each block so a busy
moment hit both alike, and each figure is the least of its runs.

**The profile before,** the wavelets alone on the 45 MP frame at six
threads, seven scales, 12.4 s in all (load 20):

| phase                              | ms     |
|------------------------------------|-------:|
| forward transform                  |     31 |
| rotation to luma and chroma        |     19 |
| variance grids                     |     88 |
| scale 0, blur / shrink and commit  |  678 / 109 |
| scale 1                            |  847 / 101 |
| scale 2                            | 1666 / 152 |
| scale 3                            | 2291 / 162 |
| scale 4                            | 2051 / 102 |
| scale 5                            | 1955 / 111 |
| scale 6                            | 1880 / 101 |
| sum, rotation back, inverse        |     53 |

Ninety-two percent of it is the blur. Bytes touched, per level: the
blur read the frame (five rows per output row, mostly from cache) and
wrote a band buffer; the second pass read the frame, the band buffer
and the accumulator and wrote the accumulator and the frame — about
six full-frame passes of 540 MB a level, 23 GB over seven, which at
this machine's bandwidth is under a second of the twelve. The
allocation was a calloc'd frame whose page faults landed in scale 0's
commit; the transforms and the grids are a quarter of a second
together. So the memory was never the cost, and the scaling §128 read
as memory-bound was a compute kernel losing clock and cache to fewer
cores, not bandwidth. The subnormal finding below is the proof: the
same frame size, the same passes and the same bytes, and a flat frame
ran every level at half the time a noisy one took.

Where the time actually was, found one at a time:

- *A library call per tap.* The edge guard is
  `exp2(-max(0, d * 0.02 / sigma^2 - 9))`, taken 25 times per pixel per
  level: 7.8 thousand million calls at 45 MP, the same fault §128 found
  in the means. The same polynomial weight replaces it — it has exactly
  the means' form — but put into the per-pixel loop alone it came out
  *slower* (17.6 s against 12.4), which is §128's first attempt over
  again: without vectorization the polynomial's dependent chain loses to
  glibc. The gain only comes once the loop is floats in and floats out.
- *Interleaved RGB and per-pixel index arithmetic.* The blur walked
  pixels, and for each pixel 25 taps with two clamps and three loads
  each; nothing in it could vectorize. The chain now runs on three
  planes — luma and the two chromas, the rotation writing them from the
  caller's buffer — and each tap of the blur is one loop over a block's
  contiguous run of every plane, 512 pixels at a time, into per-worker
  sums. The block's stretch of the five tap rows, with the blur's reach
  either side and the row's ends repeated past it, is copied into the
  worker's scratch first, so the loop has no clamp and no edge case at
  all. On the plain x86-64 target that gives packed SSE, four wide.
  With this the chain went 12.4 s to 4.9 s, and the output was the
  polynomial's difference alone: the taps are taken in the order the
  per-pixel form took them, so the sums are the same to the bit.
- *Subnormals at the coarse scales.* What remained had a shape no
  memory model explained: 330 ms at scale 0 rising to 800 from scale 3
  on and flat after, and neither ordering the rows by residue modulo
  the spacing (so a worker's consecutive rows share four tap rows) nor
  the gathered stretches moved it. Two synthetic 45 MP frames settled
  it: a flat frame, whose guard never engages, ran every scale at
  scale 0's speed, and a frame of pure noise ran every scale at twice
  that; the data decides the cost, and in a branchless SIMD loop only
  subnormal arithmetic does that. The band's noise variance is small at
  the coarse scales, so the guard's argument is large nearly
  everywhere; the polynomial's floor is `2^-126`, right for the means,
  but here it is multiplied by a filter weight as small as `1/256` and
  then by the pixel, and those products are subnormal — microcode, a
  hundred cycles an operation. A select does not help, since both its
  sides are computed. The guard is floored at `2^-60` before the filter
  weight; against the center tap's own `36/256` a weight that small
  could change no sum by as much as a last bit, and it did not: the
  45 MP output is the same to the digit as before the floor. Every
  scale now runs at scale 0's speed, and the chain is 2.3 s.
- *The shrink.* Bilinear grid lookups, a division and four gathers per
  pixel, in a loop that could not vectorize. The grid's corner values
  are a tile column's, so they are taken once per column and the loop
  over the column's pixels is plain floats; the interpolation itself is
  the same expressions per pixel and gives the same bits (checked on
  the 45 MP frame: a maximum difference of zero). Another 16 percent.
- *The passes.* The detail is shrunk in the same sweep that blurs the
  row, so the second pass is a row copy from the band buffer into the
  frame; the band buffers are allocated once for all levels rather
  than a pair per level; the caller's buffer, read once into the
  planes, is the accumulator from then on. Together these are under a
  tenth of a second at 45 MP, which is what "memory-bound" was worth.

What did not pay and came out again: the residue ordering (3.56 s
against 3.57 on an A/B once the subnormals were gone), and block sizes
of 256 and 1024 against 512 (within the load noise; 512 stays). The
gathered stretches did not pay on their own either, but they remove
the scalar edge path and they stay.

**The chain after,** same frame, same threads (load 6):

| phase                              | ms  |
|------------------------------------|----:|
| forward transform                  |  31 |
| rotation to luma and chroma        |  32 |
| variance grids                     |  42 |
| accumulator and band buffers       |  15 |
| scale 0 to 6, blur and shrink each | 272 to 318 |
| commit each                        | 6 to 12 |
| sum, rotation back, inverse        |  52 |
| in all                             | 2285 |

**The denoise alone,** six threads pinned, least of five, from
`target/work/final.log` of the branch's own session (load 7 to 10, other
agents building), with the reviewer's figures on a quiet machine
(load 3 at the start, the runs' own 6 to 7 during) and §128's before
figures:

| denoise alone, 6 threads    | wavelets | means  | hybrid |
|-----------------------------|---------:|-------:|-------:|
| 45 MP, §128 before          | 12.12 s  | 4.39 s | 18.02 s|
| 45 MP, this session, load 7 to 10 | 2.70 s | 5.11 s | 7.93 s |
| 45 MP, reviewer, quiet      | 2.25 s   |        | 6.62 s |
| 24 MP, this session, load 7 to 10 | 1.44 s | 2.66 s | 4.09 s |

The reviewer's before chain alone was 12.00 s, and every scale 285 to
327 ms after. The chain is now a third of the hybrid where it was
three quarters, and the means are the larger half again.

**End to end,** whole `greycard develop` with `--sharpen --denoise` at
their defaults and a PNG preview, the before binary built from master
and the after from this branch, alternated, five runs each, min and
median. The reviewer's machine was quiet (load 3.2 at the start, 6.3
to 7.4 during the six-thread blocks, the runs' own threads); the
session's own blocks, in `target/work/final.log`, ran under other
agents' builds at load 7 to 32 and agree on the ratio but not on the
seconds.

| develop, min / median, reviewer | before        | after         |
|---------------------------------|--------------:|--------------:|
| 45 MP, 6 thr                    | 23.59 / 25.96 s | 12.67 / 12.68 s |
| 24 MP, 6 thr                    | 10.72 / 11.34 s |  5.73 / 5.74 s |
| 45 MP, 32 thr                   |  8.12 / 8.19 s  |  5.39 / 5.41 s |
| 24 MP, 32 thr                   |  4.05 / 4.05 s  |  2.80 / 2.80 s |

The session's `final.log` at 45 MP and six threads: 25.93 / 26.12
before, 12.64 / 12.77 after (load 7 to 10); an earlier three-run block
at load 7.4 got 22.41 / 22.45 and 11.00 / 11.02, the before matching
§128's 22.56 but the after a best case that does not reproduce at rest.
So: 2.0x at six threads, 1.5x at the full count where the base develop
is the larger share. The 45 MP six-thread figure §128 named, 22.6 s, is
12.7 s; the 24 MP figure was 11.8 s and is 5.7.

**What the output moved by.** Everything in the rewrite is exact but
the guard's polynomial: the reviewer confirmed the floor is bit-neutral
(`FLOOR` at 126 gives exactly zero difference, and the scales climb
back to 4.39 s), and the gather and the corner hoist bit-exact at every
odd size in debug. Against the old chain's output on the real 45 MP
frame, the wavelets' largest difference in any of the 134 million
samples is 4.2e-7 (a value of 0.66), the mean absolute difference
3.9e-10, and no sample moved by 1e-6; the hybrid's worst is 2.3e-6
there (two samples over 1e-6), and on a second R5 frame (066A3439) the
hybrid's worst is 2.1e-6 with 39 over and the wavelets' 1.3e-6 with
five — all an order of magnitude under a 16-bit step of 1.5e-5. The
16-bit TIFFs differ by at most one step, 8-bit crops are byte-identical,
and the 8-bit previews the CLI writes differ from the old binary's on
491 of the 45 MP frame's 44.8 million pixels and 72 of the 24 MP
frame's 24 million, each by one step. Resident memory is unchanged. A golden test pins 24
samples and the mean of the wavelets' and the hybrid's output on a
synthetic frame with an edge strong enough to engage the guard, to
1e-5, values taken from the old chain with the `exp2` guard; the
in-place chain is also checked to the bit against a plain per-pixel
form on planes, over several bands and several blocks.

**What is left.** The taps are four wide because the crate compiles
for base x86-64; an AVX2-and-FMA path behind a feature check would be
worth about as much again on this desktop and nothing on the Mac §90
is aimed at, and it is the same first x86 assumption §128 declined.
The means are now the larger half of the hybrid again (about 4.4 s of
the 45 MP denoise at six threads against the chain's 2.3), and their
symmetric-offset halving from §128's "what was left" is the next
second. An algorithm change was not measured or made: with the chain
at 2.3 s the levels are equal in cost, so dropping the coarsest would
buy a seventh of it, and a cheaper wavelet or threshold would change
the look for a fraction of that; none of it would gain what the chain
work did, so nothing is proposed for the reference frames.

**The review.** The code held on every check: the floor's
bit-neutrality by flipping it off and diffing (exactly zero), the
gather and the corner hoist bit-exact at fourteen odd sizes in debug,
the golden test's values reproduced in the tree the exp2 guard still
stood in, resident memory unchanged, the 16-bit exports within a
step and 8-bit crops byte-identical. What the review corrected was
the write-up: the first draft led with a 22.4 to 11.0 s pair that was
the best case on both sides and did not reproduce at rest, quoted a
denoise-alone table not in the session's own log, said "no sample
over 1e-6" where that held for the wavelets only, and put §128's
correction softly. The reviewer waited for the machine to go quiet
and alternated the binaries five times each, which is where the
numbers above come from.
