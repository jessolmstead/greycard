# 165. The tile-write audit: what `noalias` does and does not buy (2026-09-24)

The backlog's Engine line from §128, run overnight in wave B by an opus
author and read twice by an opus reviewer who reran §128's own
benchmark.

§128 put a quarter of the non-local means' speed down to one thing:
the frame's rows reached the accumulation as `&mut [f32]` read out of
a slice, a reference loaded from memory carries no `noalias`, and the
compiler had to assume each write might change what the loop read
next. The fix was to keep the sums in `accumulate`, `#[inline(never)]`,
and write the frame from its caller. This entry checks that claim
against what the compiler actually emits, and audits every op that
writes rows or tiles straight into a frame for the same shape.

**The correction to §128, in short.**

The mechanism §128 named, a `&mut` into the frame live across the
loop, costs nothing when the loop's inputs and scratch are arguments:
no write through the frame's rows can touch an argument, and the loop
compiles the same either way. What costs is state read through the
closure's captured references — a model's fields, a pattern's size, a
kernel, a slice's length — which the compiler must read again after
every write through a row whose pointer it loaded from memory. That is
where every gain below came from. §128's own quarter was measured on an
earlier shape of the tile code and does not reproduce on the current
one (same toolchain, rustc 1.98.1, installed 2026-09-05, before §128).

**What the compiler does.**

A scratch crate (rustc 1.98.1, plain x86-64, release; not kept in
the tree) set the shapes side by side and read the IR and the assembly:

- A `&mut [f32]` argument is `noalias` in the IR. The rows rayon hands
  a closure from `par_chunks_mut(w).enumerate()` are not: the item is
  the tuple `(usize, &mut [f32])`, 24 bytes, which the Rust ABI passes
  by pointer, so the slice's pointer is loaded from memory and carries
  nothing. The same holds for any `zip` of two slices (32 bytes). A
  plain `par_chunks_mut(w).for_each(|row| ..)` row is a scalar pair,
  passed in registers, and is `noalias`.
- That alone costs nothing when everything else the loop touches is an
  argument: an accumulation over offsets into a scratch row, written
  out after, compiles to the same packed loop whether the frame's rows
  come from a `&mut [&mut [f32]]` or from an argument (`k1_rows_inline`
  against `k1_rows_outline` and `k1_direct`): the scratch and input are
  `noalias` arguments, so no write to the frame can touch them.
- What it does cost is state read through the closure's captures — a
  model's fields, a pattern's width, a slice's length. Those pointers
  are loaded too, so after every write to a row whose pointer is also
  loaded the compiler must read them again. LLVM says so in its
  remarks, "failed to move load with loop-invariant address because
  the loop may invalidate its value", and it is where the gains below
  come from.

**§128's own case does not reproduce.** Putting `accumulate` back
inline (`#[inline(always)]`) into `denoise_tile`, beside the write-out
through the frame's rows, and emitting the assembly of both builds: the
means' two hot loops, the weight loop (41 instructions, four weights at
once with `maxps`/`minps` and eleven `mulps`) and the accumulation loop
(84 instructions, four pixels at once), are the same instruction
sequences in both. Both run behind the same runtime overlap check,
because `acc`, `wrow`, `hsum` and the ring are vectors inside `&mut
Scratch` and their pointers are loaded; only the scalar fallbacks
differ, by two register moves. And the time does not move:

| non-local means alone, 45 MP | build           | min     | median  | runs | load      |
|------------------------------|-----------------|--------:|--------:|-----:|-----------|
| 4 threads, cores 4-7         | out of line     | 6.597 s | 6.684 s | 5    | 9.8-14.4  |
| 4 threads, cores 4-7         | inlined         | 6.423 s | 6.455 s | 5    | 9.8-13.5  |
| 1 thread, core 10            | out of line     | 24.77 s | 28.85 s | 5    | 9.6-15.9  |
| 1 thread, core 10            | inlined         | 24.23 s | 26.72 s | 5    | 10.7-17.7 |

The reviewer reran §128's own instrument, `nlm_bench 1500 1000 1`,
one thread pinned, seven rounds alternating: outlined 0.812 / 0.831 s,
inlined 0.832 / 0.840 s (min / median), the same checksum. So the split
is harmless and does no work. The quarter was measured on an earlier
shape of the tile code, which is not in the history; the toolchain is
the one §128 used. The comment on `accumulate` now says so, and the
outlining stays.

**How the ops were timed.**

`develop::timing::ops_alone`, an ignored test: the 8192x5464 R5 II frame
`066A3439.CR3` is decoded, prepared and demosaiced once, then each op
runs on a fresh copy of its input, the copy untimed, and prints its
time, the one-minute load and a digest of its output's bits.

```
GREYCARD_RAW=066A3439.CR3 GREYCARD_OPS=dehaze GREYCARD_RUNS=5 RAYON_NUM_THREADS=1 \
  taskset -c 10 cargo test --release -p greycard-core --lib -- --ignored --nocapture ops_alone
```

Before and after builds were run alternately, one run each per round,
so a busy moment hit both. Other agents were
building on the machine throughout and the load ran from 3 to 35; one
thread pinned is the least disturbed and the most sensitive to code
generation, and the least of the runs is the figure to compare. Every
figure has its thread count beside it: the lens, the RCD's own-row try
and the RCD inlining's second row are four threads on cores 4-7 (the
inlining rows on their own cores and load), everything else one thread
on core 10. The least of a build's runs moved by 1 to 3 percent between repeats; a
change is kept only where it beat that in every repeat and the digest
was the same, which it was for every op before and after.

**The audit.**

"Shape" is what the loop does: *captured* means it writes a row whose
pointer is loaded (an `enumerate` or `zip` tuple, a vector of rows)
while reading state through the closure's captures; *none* means the
row is an argument, or nothing but the row is read after a write, or
the loop cannot vectorize or hoist anything for other reasons.

| op (site)                                         | shape    | before (min / median) | after (min / median) | threads, runs, load | kept |
|---------------------------------------------------|----------|----------------------:|---------------------:|---------------------|------|
| `lens::correct`, resample                          | captured | 2.109 / 2.152 s       | 1.743 / 1.767 s      | 4, 7, 8.9-14.7      | yes  |
| `lens::correct`, vignetting only                   | captured | 0.201 / 0.207 s       | 0.049 / 0.051 s      | 4, 7, 8.9-14.7      | yes  |
| `dehaze` (whole op; the clearing loop changed)     | captured | 0.372 / 0.392 s       | 0.352 / 0.363 s      | 1, 10, 6.0-13.8     | yes  |
| `apply_gains_cfa`                                  | captured | 0.0987 / 0.1000 s     | 0.0518 / 0.0534 s    | 1, 10, 6.4-35.5     | yes  |
| `normalize_levels`                                 | captured | 0.212 / 0.230 s       | 0.160 / 0.181 s      | 1, 10, 6.4-35.5     | yes  |
| `dual::gaussian_blur`, both passes (repeat 1)      | captured | 0.147 / 0.154 s       | 0.121 / 0.127 s      | 1, 7, 5.3-25        | yes  |
| same (repeat 2)                                    |          | 0.142 / 0.166 s       | 0.134 / 0.154 s      | 1, 10, 6.4-35.5     |      |
| same (repeat 3)                                    |          | 0.144 / 0.150 s       | 0.134 / 0.136 s      | 1, 7, 6.8-34.7      |      |
| `demosaic_bilinear`                                | captured | 0.652 / 0.681 s       | 0.657 / 0.707 s      | 1, 10, 6.4-35.5     | no   |
| `rcd`, `own_row` around its five passes            | captured | 1.664 / 1.685 s       | 1.638 / 1.717 s      | 4, 7, 4.9-9.1       | no   |
| `rcd`, `direction_stat` always inlined (see below) | —        | 4.541 / 4.542 s       | 4.015 / 4.031 s      | 1, 6, 1.0-1.3       | yes  |
| same                                               |          | 1.400 / 1.404 s       | 1.268 / 1.275 s      | 4, 6, 1.2-2.5       |      |
| `highlights::inpaint_opposed`, fill                | captured | 0.0620 / 0.0647 s     | 0.0622 / 0.0722 s    | 1, 10, 6.4-35.5     | no   |
| `segments`, replace and `morph`                    | captured | 0.162 / 0.224 s       | 0.160 / 0.164 s      | 1, 7, 5.3-25        | no   |
| `ca`, color-shift factor and `transpose`           | captured | 2.146 / 2.445 s       | 2.158 / 2.292 s      | 1, 7, 5.3-25        | no   |
| `defringe`, the replacing pass                     | captured | 3.320 / 3.514 s       | 3.261 / 3.479 s      | 1, 7, 5.3-25        | no   |
| `local_contrast`, `box_mean` column pass           | captured | 1.209 / 1.275 s       | 1.210 / 1.430 s      | 1, 7, 5.3-25        | no   |
| `sharpen`, `blend_mask` and `dilate_zeros`         | captured | 5.821 / 6.331 s       | 5.722 / 5.794 s      | 1, 7, 6.1-34.7      | no   |
| `dual`, blend-in and `blend_mask`                  | captured | 8.139 / 9.864 s       | 8.144 / 8.466 s      | 1, 7, 6.1-34.7      | no   |
| `sharpen`, Richardson–Lucy tiles                   | none     | —                     | —                    |                     | —    |
| `profile::MapStage::apply`                         | none     | —                     | —                    |                     | —    |
| `picture::to_working`                              | none     | —                     | —                    |                     | —    |
| `guided::box_mean`, `dehaze` `min_filter`/`box_mean` | captured | not timed alone     |                      |                     | no   |
| `vng4`, `amaze`                                    | none     | —                     | —                    |                     | —    |
| `greycard-edit` brush `stamp`                      | none     | —                     | —                    |                     | —    |

The sharpen, dual and defringe rows time the whole op, and their after
builds carried the blur's change too, which all three call; so the
little they gained is at most the blur's share, and their own loops
add nothing past the noise.

What the kept ones do, and why they gain where the others do not:

- **Lens.** The two row loops are now `vignette_row` and `resample_row`,
  `#[inline(never)]`, taking the row, the image and the correction as
  arguments. The vignetting loop was scalar and re-read the model for
  every pixel; now the whole per-pixel gain, square root and
  polynomial, runs four pixels at once (the line that multiplies went
  from one pixel's three channels an iteration, `mulps` on two and
  `mulss` on the third, to a packed body over four pixels with the RGB
  interleave undone in shuffles). Four times
  faster at four threads. The resample keeps its scalar cubic gathers but no longer
  reloads the distortion and CA coefficients per channel: 17 percent at
  four threads.
- **Dehaze.** The clearing loop is `clear_row`, out of line, the row and
  the map's row as arguments. The transmission's bilinear taps read
  `model.a`, `model.b`, the width and the airlight, all reloaded per
  pixel before. Five percent of the whole op at one thread, which also reduces,
  estimates and guided-filters; the loop's own share is larger.
- **Gains, levels and the blur.** Wrapped in `develop::own_row`, an
  `#[inline(never)]` generic that takes the row and a closure, so the
  row is `noalias` and the closure's body is inlined into it. In
  `apply_gains_cfa` the pattern's height and width were read back after
  every sample and `y % height` done per sample; the assembly after
  hoists the row's term and keeps the column's in a counter. Twice as
  fast at one thread. `normalize_levels` is the same through the level
  patterns, a quarter off at one thread. The blur's passes re-read the
  kernel and the row bounds; 6 to 18 percent off at one thread over
  three repeats.
- **RCD.** Not an aliasing matter. A build made during the audit
  (all the row wraps, RCD's own code untouched) ran RCD in 4.55 s at
  one thread against 5.54 s for the build before it, bits the same,
  and a rebuild of the same wraps on a later tree was back at 5.5 s.
  The symbols say why: in the slow builds pass 1's folder calls
  `direction_stat` out of line twice a pixel (940 bytes, 2 scalar float
  ops in the body), in the fast ones it is inlined (2679 bytes, 212).
  It is `#[inline]`, and LLVM's decision flips from build to build as
  unrelated code in the crate changes. `#[inline(always)]` makes it
  inlined in pass 1 and in pass 4a too, where no build had inlined it:
  4.02 s against 4.54 s for the fast layout at one thread, 11 percent,
  and 27 percent against the slow one; 1.27 s against 1.40 s at four
  threads. The digest is the same, e6799512fa93b0fc, in every build.
- **What does not gain.** Where the per-pixel work is long and scalar
  whatever happens (RCD's stencils, the bilinear's neighborhood walk,
  the highlights' sparse fills, the segments' morphology with its early
  exits, defringe's window sums), a reload or two per pixel is lost in
  it. Where the loop already walks iterators over slices it sliced
  before the loop (the box means, the CA factor maps), the pointers are
  already in registers.

**What is left.**

- The wavelet chain in `denoise.rs` was not touched; its owner has this
  entry's finding.
- `stack.rs` and `register.rs` write rows the same way (warps and
  resampling with captured models) and were not in this audit's list;
  the lens result suggests their warps are the next place to look.
- `own_row` is kept only where it measured; it is not a rule to apply
  everywhere, and the bilinear and RCD rows show why.

**The review.** The reviewer reproduced every kept gain alternating
the binaries, confirmed every changed op bit-identical and a full
develop pixel-identical, reran the scratch crate's IR and assembly
comparison, and then reran §128's own instrument to settle the
quarter. It also caught what the first draft had wrong: RCD listed
under "no gain" when the author's own scratch binary was 17 percent
faster on it, which led to the inlining finding above; a dehaze gain
at 32 threads that was noise; and the `own_row` doc claiming a
measurement of plain `#[inline]` that was never taken. Two comment
sentences were softened at landing: the means' scratch buffers are
behind loaded pointers, so the loops run behind an overlap check
whether or not the accumulation is out of line.
