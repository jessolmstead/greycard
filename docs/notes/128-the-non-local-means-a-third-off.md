# 128. The non-local means, a third off (2026-09-20)

The non-local means, and what its scaling really was (2026-09-20)

§90 pinned the develop to fewer threads and found the profiled
denoiser the exception: 3x slower from 32 threads to 6 where the rest
of the develop lost under 2x, and guessed at an inner loop that only
pays with many threads or at per-thread scratch whose cost does not
shrink with the count. Taken on its own, on the same desktop, the
means turn out to have no scaling fault at all. What that table was
showing is the other half of its own sentence.

The column §90 never took is the one that settles it. This is a
16-core machine with two threads a core, so 32 threads is sixteen
cores and their SMT siblings, and 16 threads is the sixteen cores.
Timing the means alone on a synthetic frame, away from the decode and
the PNG, three runs each and the median (`taskset -c` and
`RAYON_NUM_THREADS` together, release build, the before and after
builds run one after the other in each block so a busy moment hits
both alike; the load beside each block ran 4.8 to 12.4 and is mostly
the run's own threads, and the three runs of a block agreed to within
two percent, so it was not fighting anything):

| non-local means alone | 32 thr | 16 thr | 10 thr | 6 thr |
|-----------------------|-------:|-------:|-------:|------:|
| 24 MP, 6000x4000      | 1.37 s | 1.51 s | 2.26 s | 3.69 s|
| 45 MP, 8480x5650      | 2.73 s | 3.03 s | 4.49 s | 7.35 s|

Sixteen cores to six is 2.44x for 2.67x fewer cores — slightly better
than linear, because six cores hold a higher clock — and the SMT
siblings buy ten percent on top. That is a kernel turning cores into
speed at very nearly the full rate. So in that comparison the base
develop's 1.7x from 32 to 6 is not the healthy number and the means'
2.7x the sick one; it is the other way round. The base develop
saturates its memory before it saturates sixteen cores, so taking ten
away costs it little, and §90's own reading of the 45 MP row ("the
wide run is memory-bound past a point") said as much without following
it through. A stage that is compute-bound cannot be brought under 2x
from 32 threads to 6 except by making it worse.

So the item is not a scaling fix. It is the absolute cost, which is
what the 6-thread column and §90's laptop arithmetic actually care
about.

**Where the time went.** Per pixel and per offset the tile loop does
about thirty scalar operations, and at 45 MP with the dense 15x15
window that is 10.8 thousand million of them. Two things were making
each one cost several times what it should:

- **`f32::exp2` is a call into the C library**, taken once per pixel
  per offset. A call in the middle of a loop keeps that loop scalar
  and spills around it. Stubbing the weight out for a reciprocal —
  which ruins the picture but keeps the shape of the loop — took a
  24 MP frame from 1.49 s to 0.92 s: the weight was a third of the
  whole.
- **The build is plain x86-64.** The tile loop's disassembly has no
  `ymm` or `zmm` register in it, no `vfmadd`, and `ceilf` as a call:
  the crate compiles for the base instruction set, so SSE2 four wide,
  no FMA, no `roundss`. §56 already tried `-C target-cpu=native` and
  found it a mixed result, so this is the target the code has to be
  quick on. It is also what the first replacement for `exp2` failed to
  survive: a degree-6 Horner chain with `ceil` for the rounding came
  out *slower* than glibc's `exp2f` — 1.32 s against 1.28 on one core
  — because without FMA the chain is twelve dependent operations deep
  and the rounding is another library call. What settled that was
  reading the assembly, not the clock.

Beside those, the tile's own shape: the vertical box sums ran down one
column at a time, striding a row of the difference plane per step,
which no compiler will vectorize and which read a 38 KB plane back
column by column after writing it row by row; that plane was cleared
in full for each of the 225 offsets although the offsets that matter
write all of it.

**What changed in `nlm.rs`.**

- *The box sums are row-major.* One row of running vertical sums
  slides down the tile over contiguous floats. The rows of squared
  differences are made one at a time into a ring of `2r + 2` of them,
  one more than the patch is tall, so the row entering the window and
  the row leaving it are both in hand and the running sum updates as
  `s += new - old` — which is exactly what the column loop did, so the
  answer is the same to the bit. The tile's two largest scratch
  planes, 38 KB and 37 KB, become 1.6 KB and 400 bytes at the default
  patch, and the per-offset clear becomes a clear of the uncovered
  border alone.
- *The weight leaves the accumulation.* A row's weights are computed
  into a buffer of plain floats first and the accumulation then walks
  it. Together in one loop the weight's arithmetic is stuck at the
  pace of the three-wide gather beside it; apart, the weight loop is
  floats in and floats out and the vectorizer takes it.
- *The weight is arithmetic, not a call.* `2^x = 2^n * 2^f` with `n`
  the nearest integer: the rounding is an add of 1.5 * 2^23 and a
  subtract of it again, which pushes the fraction off the end of the
  mantissa, and the same sum carries `n` in its low bits, so the scale
  `2^n` is those bits shifted into an exponent — no `ceil`, no
  conversion. `2^f` over the half unit left is a degree-6 polynomial,
  grouped in pairs so the multiplies do not queue up behind one
  another. Its coefficients are a minimax fit and not the Taylor
  series, which costs nothing and is worth a good deal: Taylor's
  seventh term, the one left off, is 1.2e-7 of the value at the worst
  `f`, and the fit is out by 2e-9, leaving only the f32 arithmetic of
  evaluating it. This is the one part of the change that is not exact.
  Sweeping every `f32` the dissimilarity can reach, the worst weight
  is 2.24e-7 out — under four of the last bits — and
  `the_weight_follows_exp2` holds it under 3.5e-7, with room for
  another platform's `exp2` to differ in its own last bit. It is not
  that the weights share an error and it cancels; the polynomial's
  error turns with the fractional part, so it does not. What holds is
  the plain bound: a weighted mean every one of whose weights is
  within a relative `d` of the right one is itself within about twice
  `d`. On the synthetic patch the new golden test pins, the worst of
  24 sampled values moved by 4.4e-7 of itself and the frame's mean did
  not move at the ninth decimal. The test's tolerance is 1e-5, twenty
  times the worst seen and well under a 16-bit sample's own step.
- *The accumulation does not see the frame.* Writing a tile's answer
  straight into the picture, rather than into a buffer of its own that
  is then copied, needs the tile's rows as `&mut [f32]` read out of a
  slice — and a reference loaded from memory carries no promise that
  it is distinct from anything else, so with the write folded in
  beside the sums the compiler must allow that those rows are the
  input or the scratch. That cost a quarter of the speed and it does
  not show in any profile as anything but a slower loop. The sums live
  in `accumulate`, which is `#[inline(never)]` and takes no `&mut` the
  frame is reachable through, and the writing out is its caller's.
- *The scratch is kept.* Each rayon worker gets one `Scratch` for the
  run of tiles it takes rather than a hundred and fifty kilobytes
  asked of the allocator per tile. The ring index is walked rather
  than taken modulo, since a modulo by a number only known at run time
  is an integer division in the middle of the loop.

The same table after:

| non-local means alone | 32 thr | 16 thr | 10 thr | 6 thr |
|-----------------------|-------:|-------:|-------:|------:|
| 24 MP, 6000x4000      | 0.89 s | 0.90 s | 1.40 s | 2.27 s|
| 45 MP, 8480x5650      | 1.79 s | 1.80 s | 2.78 s | 4.51 s|

A steady 1.6x at every width, and the 6-thread column, the one the
laptop arithmetic multiplies, comes down by nearly two fifths. The
scaling is where it was and where it should be: 32 to 6 is 2.5x
against 2.7x, and the SMT siblings now buy nothing at all, where they
bought ten percent before. That fits: what a sibling used to fill was
the stall around the library call, and there is no call left. Cores 0
to 5 are all on one core complex of this 9950X3D and 0 to 9 straddle
the two, one of which carries the stacked cache, so the 10-thread
column is not quite comparable with the others in either table.

**End to end.** The same runs §90 timed, a whole `greycard develop`
with the decode and the PNG in them, least of five with the machine to
itself and the before and the after run one after the other. The base
develop rows, whose code did not change, come out within a percent
either way, which is the precision here:

| develop, least of five           | 32 thr  | 10 thr  | 6 thr   |
|----------------------------------|--------:|--------:|--------:|
| 24 MP, sharpen                   |  1.43 s |  1.83 s |  2.34 s |
| 24 MP, sharpen + denoise, before |  5.30 s | 10.31 s | 15.51 s |
| 24 MP, sharpen + denoise, after  |  4.30 s |  8.13 s | 11.81 s |
| 45 MP, sharpen                   |  2.73 s |  3.41 s |  4.53 s |
| 45 MP, sharpen + denoise, before |  9.94 s | 18.95 s | 28.29 s |
| 45 MP, sharpen + denoise, after  |  8.20 s | 15.40 s | 22.56 s |

The 24 MP 6-thread row before is 15.5 s against §90's 15.0, so this is
the same measurement on the same machine; it is 11.8 s now. The frames
are a 6000x4000 R6 Mark II and an 8192x5464 R5 Mark II, `--sharpen`
and `--denoise` at their defaults, a PNG preview written each time.

The gain is bigger on a photograph than the bench predicts, and by a
lot: pinned to six threads, the means' own stage at 45 MP goes 10.24 s
to 4.39 s, 2.3x, where the bench at that width and that thread count
said 1.6x. The synthetic frame is smoother than a picture, so more of
its weights fell under the floor and took the `d <= 0` branch that
never called `exp2` at all. The bench is the conservative figure and
the table is the real one.

**What is left of the 7 s.** §56 left the profiled denoiser at 7 s on
the 45 MP frame at 32 threads. Splitting it by method is clearest
where the machine is steadiest, six threads pinned to one core
complex, least of five:

| 45 MP develop, 6 threads    | before  | after   | the denoise alone |
|-----------------------------|--------:|--------:|------------------:|
| no denoise                  |  4.53 s |  4.54 s |                   |
| `--denoise-method nlm`      | 14.77 s |  8.93 s | 10.24 -> 4.39 s   |
| `--denoise-method wavelets` | 16.48 s | 16.65 s | 11.95 -> 12.12 s  |
| hybrid, the default         | 28.29 s | 22.56 s | 23.76 -> 18.02 s  |

The means were 43 percent of the profiled denoise and are 24 percent
of it now. Everything else in it — the à trous shrinkage over six
scales, the variance grids it wants, the rotation to luma and chroma,
and the second run of the means over a 512-square field of noise to
measure what they left — is the other three quarters, and none of it
was touched here.

**What was left.** The search offsets are symmetric and so is the
patch dissimilarity: `w(p, p + o)` and `w(p + o, p)` are the same
number, so half of the difference work, the box sums and the weights
are done twice over. Taking it means accumulating into pixels outside
the tile the weight was computed in — darktable's scatter, which wants
a whole-frame buffer per offset and gives up the tiling that keeps
this in cache. It is worth close to another factor of two and it is
its own item. A runtime AVX2-and-FMA path behind a feature check would
be worth as much again on this desktop and nothing on the Mac §90 is
aimed at, and it would be the first x86 assumption in the engine; also
left. The tile side stays at 96.

**And what the roadmap should say now.** Not a scaling line. The
profiled denoise at 45 MP on six threads is 22.6 s from 28.3, and at
24 MP 11.8 s from 15.5; that is the figure a tester can check and the
one the laptop arithmetic multiplies. What is left is the wavelet
chain, and the shape of it is the opposite of the means': the
wavelets scale the way the base develop does, 7.25 s to 12.5 s from 16
cores to 6, 1.7x for 2.7x fewer, so they are memory-bound and more
cores will not help them either. They are now about three quarters of
the profiled denoise at six threads, and the next second to be had in
the denoiser is in them.

**Seeding, corrected the same evening.** The first version seeded the
three presets only when the preset directory did not exist yet, on
the reasoning that afterwards the store is the user's. On the one
machine that matters the directory was there already, with the
user's own presets in it, so nothing was seeded and the presets were
nowhere to be found. A store that existed before a preset shipped is
the common case, not the exception. The record of what has been
seeded is now a `seeded` file in the store listing the stems put
there so far: each shipped preset is written once, when its stem is
not on the list and no file of that name is there, and a user's file
under a shipped name stands. One binned stays binned, as before,
because its stem stays on the list.
