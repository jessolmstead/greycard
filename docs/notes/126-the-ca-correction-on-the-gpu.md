# 126. The CA correction on the GPU (2026-09-20)

The CA correction on the GPU (2026-09-20)

The roadmap line had the color-shift guard's blur dominating the CA
correction's 234 ms (§56) and asked for a partial port: the guard's
blur and the per-tile votes on the GPU, the median, the gate, the fit
and the solve on the CPU, the resample wherever it paid. Timed stage
by stage first, the split has moved since §56's transposes: on the
45 MP R5 II frame each of the two passes is 33 ms of votes and 63 of
resample (the median, gate, fit and solve under 2 ms), and the guard
is 57 (factors 16, the blur 33, the multiply 8); on the 24 MP R6 II
frame 20 and 35 a pass, the guard 27. The resample is half of it, the
blur a seventh. So the port covers every data-parallel stage, and
what stays on the CPU is exactly the decisions: `fit_votes`, which is
now one shared function holding the median, the gate, the normal
equations in double and the solve, called on the votes read back.

**No atlas.** The reference works in tiles of 128 with a border of 8
it computes and throws away, reflecting reads past the picture's
edge, and the sharpen's port kept that shape in an atlas. The CA's
does not need it: every value a tile's interior reads is determined
by the picture alone. The interpolated green is read within four
pixels of the interior, the border covers that, and the sign of a
fitted shift and the direction of the resample's neighbor are
correlated so that the reads stay on the interior's side of the
border in every case. So the green is one `R32Float` plane padded by
the border on every side, computed at every position by the
reference's formula on the reflected mosaic; the tiles' interiors are
the picture cut into 112-pixel blocks; the vote is a workgroup a tile
over its block; and the resample is a thread a pixel with its tile's
parameters, the tile found from the pixel's position. The mosaic
and its two corrected versions (the passes alternate between them,
the upload kept for the guard) are full planes, the padded green
another, the factors and the blur's scratch half-size planes: 812 MiB
at 45 MP. They are made for the run and dropped at its end. The op
runs once a base develop and uploads its mosaic each time, so there
was nothing to keep between runs, and the first version's holding
them for the session (the review measured the editor's process at
2841 MiB on the device during a 45 MP export) was a habit borrowed
from the sharpen, whose working set a slider re-reads. With the
planes dropped the same export peaks at 2021 MiB, sampled at ten a
second. The default device limit of four storage textures a stage
meant two bind group layouts for one shader, the passes' and the
guard's, each kernel under the one it uses.

**The sums in the reference's order.** The votes are twelve
single-precision sums a tile over some six thousand terms each, in
row-major order, and the variance gate reads their quotients. A tree
reduction would put the two paths a different rounding apart at
every tile. Instead each chunk of four interior rows has its sites'
terms made by the workgroup into workgroup memory, and one thread
adds them in the reference's order; the terms are rounded to single
before the add, as the reference rounds them, and no multiply-add
fusion can reach across the store. The guard's blur is the
reference's running sum, a thread a line for its three row passes
and three column passes (the columns without the transpose, which
was only ever the CPU's way to a row), with no multiply to fuse. The
per-tile resample parameters (`resample_for`: floor, ceil, fraction
and direction per color) are made on the CPU in double from the fit
and uploaded, 64 bytes a tile. What is read back between the stages
is twelve floats a tile, 186 KB at 45 MP.

**How far apart.** The tests (`crates/greycard-gpu/tests/ca.rs`)
print what they measure, at three levels. The votes first, tile by
tile (`measure_votes` on the reference against `Context::ca_votes`):
the shifts within 4e-5 px on the synthetic mosaics, 2.5e-4 on the
24 MP frame and 1.6e-5 on the 45 MP, the weights within 4e-5
relative; a tile with little to vote on divides two small sums, and
the 2.5e-4 is one of those. Then the gate, which is a cliff (one tile
crossing it changes the fit and so every pixel): a `Fit` now carries
how close its nearest tile came, as a fraction of the gate, and the
tests hold the two paths' margins to a tenth of each other beside
the equal block counts. They are 0.75 on the synthetic mosaics
(nothing near), 8.4e-3 on the 24 MP frame and 1.4e-3 on the 45 MP,
where the votes' differences move the median-squared shift by
2e-4 and 1e-4 of the gate: a factor of forty and fourteen short of
a flip, and visible now rather than hidden in a count. Then the
pixels. After one pass the two paths are 4e-6 relative apart at
worst, mean 4e-8, with the decisions (corrected, the tiles that
vote, the fit's order) equal and the largest fitted shifts within
4e-6 px. The second pass widens the worst samples to a few 1e-5: it
reads the first's result through the reference's weights
`1 / (EPS + |g0 - gs|)` with EPS at 1e-5 in units where the mosaic
is 0..1, which turns a last bit in a flat patch into a percent of a
weight. And a handful of samples take a different branch of a
per-pixel guard (which candidate wins, whether the correction
overshot): one to three in a few hundred thousand on the synthetic
mosaics, 16 of 24 million on the R6 II frame, 9 of 45 million on the
R5 II. Every candidate the guard chooses between is within the
correction of the sample, so such a step is bounded by the larger
correction the two paths applied there: the tests measure it at 0.14
to 0.99 of that (the flip between keeping and correcting is 1.0) and
hold it to 3. The tolerance held for the rest is 1e-4 relative to
the sample, the relative difference floored at 1e-5 rather than the
1e-3 the first version had (which let a difference four orders above
a dark sample pass as a step); the steps are counted per test (3 on
the synthetic mosaics, 20 and 50 on the frames, from the measured 16
and 9 with headroom), and the mean must be under 1e-6 (measured
5.6e-8 and 1.5e-8). The refactor of the reference that made its
pieces shared is bit-identical on the 24 MP frame's TIFF (the eight
bytes that differ are the timestamps).

**The export stays the reference's.** The sharpen's port could leave
the export alone because the export re-runs the sharpen on the CPU
from the cached base; the CA is in the base. The first version
passed no context to the export's develop and still handed it the
session's base, GPU CA and all (the review measured a default export
against a `--cpu-ops` one: peak 8 of 255 at 59 pixels of 24 million).
Decided: the export's picture is the reference's, at the cost of a
base develop. A `Base` records whether its CA ran on the GPU, and a
develop without a context (the export's) does not reuse one that
did: it makes the base afresh on the CPU, and the session keeps that
base, so a second export pays nothing more and a later viewport
develop under the same base edit reads the reference's. The cost is
one base develop on the first export after a GPU develop: 2.7 s on
the 24 MP frame and 2.8 s on the 45 MP here under a load of twenty
from other builds (1.0 and 1.9 s quiet), on an export that takes
seconds anyway. Checked two ways: a worker test develops a synthetic
raw with a context, then without, and holds the second's picture
byte-equal to a session that never had a context; and the editor's
own `--export` of the 24 MP frame, default against `--cpu-ops`, has
identical strip data in the two TIFFs (3 bytes differ, the
timestamps). The log's second develop line says so too: `base made,
CA 0.36 s, sharpen`.

**Errors.** A device error in the op (the tests provoke one on a
device of their own allowed twenty workgroups a dimension, which a
400x320's twenty-six exceed; the compute pass's validation error is
caught by the op's error scope and comes back as `Error::Gpu`, and
the context runs a 300x300 afterwards) falls back to the reference
for that develop and drops the context for the session, as the
sharpen does. A picture the device's textures cannot hold (the CA's
green plane is sixteen wider than the picture, so a device whose
textures stop at 512 holds a 500-wide picture and not its plane) is
`Error::Unsupported`, refused before any GPU work, and the worker
falls back to the CPU for that op and keeps the context; the first
version dropped it, taking the sharpen with it. Both paths have a
worker test, driven without a window.

**How much faster.** The op alone, `examples/bench.rs`, release, the
16-core desktop against the RTX 5070 Ti, in the quietest moment the
day had (three other agents building; load 4.6 to 4.8 for these
runs, 7 to 30 the rest of the time): 45 MP 284 to 330 ms on the CPU
against 94 to 104 on the GPU; 24 MP 128 to 136 against 47 to 49. The
review's own runs had 286 to 290 against 73 to 82 at 45 MP. An
earlier pair of 225 against 72, taken at load 7 to 10, did not
repeat and is not the figure. Under load 20 to 25 the CPU's 45 MP
runs spread from 245 to 890 ms while the GPU's stayed at 88 to 100
(180 once): the CPU figure moves with the load and the GPU's does
not, which is the point of the port for a laptop. Of the GPU's time
at 45 MP, timed stage by stage once: the upload about 9 ms, the
green and the votes 12 a pass, the read back and fit 2, the resample
1, the guard 10 (its blur's twelve passes run one thread a line, a
few thousand threads for a few thousand steps), and reading the
corrected mosaic back 27, a third of the whole. In the editor (`-v`,
`--screenshot`, load 10 to 16): the 24 MP frame's first develop went
from 0.82 s to 0.75 (its CA from 0.13 s to 0.06), the 45 MP frame's
from 1.55 to 1.42 (0.24 to 0.11); the CPU reference under `--cpu-ops`
logs its own time the same way. So the base develop is a tenth
quicker and the CPU is free of the CA for it; the rest of the base
is the demosaic.

**How it is wired.** greycard-core's `prepare` and `develop` have
`prepare_with` and `develop_with` beside them, which take an optional
`CaCorrector`: a function with `correct_ca`'s signature that runs in
its place. No wgpu in core. The worker hands the context's
`correct_ca` in for the base develop (the engine's and the learned
denoiser's alike); the export passes nothing. The log's develop line
says `CA on the GPU 0.06 s` or `CA 0.13 s`. The shader and its
driver name RawTherapee's `CA_correct_RT.cc` and its authors in
their headers, as the reference does.

**Left out.** The read back of the corrected mosaic is the largest
single cost and could be halved by keeping the mosaic on the device
for a demosaic that ran there, which is the next op. The blur's low
parallelism is fine at 10 ms but a scan would be the way if it ever
mattered. The CLI still has no `--gpu`. And the first export after a
GPU develop pays a base develop; keeping a CPU base beside the GPU
one would cost the memory of a second base for a second that the
export's own seconds hide.
