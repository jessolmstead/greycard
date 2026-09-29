# 118. The sharpen on the GPU (2026-09-20)

The sharpen on the GPU (2026-09-20)

The Engine item "GPU implementations of engine ops" had waited on the
editor showing which ops must be interactive, and §90 named them: the
sharpen, which re-runs on its slider at 600 ms here and seconds on a
laptop, and the CA correction on every base develop. This is the
sharpen. The CA correction is not here, and the end of this section
says what shape it wants instead.

**A crate of its own.** `greycard-core` must never depend on wgpu, so
the GPU ops live in `crates/greycard-gpu`, GPL like the rest, which
depends on greycard-core and on wgpu at the major Slint 1.18 pins
(30, §91): the editor hands the crate its own device and queue, and
the two must be one wgpu or the texture an op leaves could not be
the viewport's. A `Context` is built either from that borrowed pair
(`from_device`, the editor) or from an adapter of its own (`own`,
the CLI and the tests; `NoAdapter` when the machine has none). One
function per op with the reference's semantics: `Context::sharpen`
takes an uploaded picture, the reference's `SharpenOptions`, the
measured radius and the clip level, writes an `Rgba16Float` texture
with the blend mask in its alpha, and hands back the reference's
`SharpenStats`; `sharpen_image` has the CPU function's exact
signature, in place on a `WorkingImage` with the mask returned, for
the tests and anything that wants the pixels back. The CPU path
stays the reference and the fallback: the export develops on it
whatever the viewport ran on, and `--cpu-ops` on the editor keeps
every op there for checking one against the other.

**Errors are errors.** wgpu's uncaptured-error handler panics, and
on the editor's device it is Slint's to set, not ours. So every
upload, run and read back is wrapped in error scopes (validation,
out of memory, internal), popped in reverse before the call returns,
and whatever the device rejected comes back as `Error::Gpu` naming
what was being done. A test hands the op a texture without storage
usage, which fails the bind group's validation, and asserts the
error and that the context still works after it. In the worker one
such error, or a picture larger than the device's texture side, is
warned once, the context is dropped with its working textures, and
every develop from then on is the CPU's: a 2 GB allocation that
fails on a small card must not become a develop that fails on every
slider.

**Textures, not storage buffers.** The editor creates Slint's device
with wgpu's default limits (only the texture side raised to 16384,
§14), and a storage buffer binding is then 128 MB at most. A 45 MP
plane of floats is 180 MB. Rather than raise limits the editor
cannot ask the adapter about before Slint creates the device, every
plane is an `R32Float` texture, which is limited by its side and not
its bytes, read and written as `read_write` storage (a baseline
capability for the 32-bit single-channel formats). The uploaded
picture is `Rgba32Float`, full floats, because the op has to read
what the reference reads: an input quantized to halves would put
the two paths 1e-3 apart before a single iteration, and the
viewport's own half-float quantization is accepted only at the
output, where §18 already measures it. The upload interleaves to
four channels a band of 32 MB at a time and submits after each
band, since the queue holds every `write_texture`'s staging copy
until the next submit; without the submits a 45 MP frame would hold
720 MB on the CPU and as much again in staging while the copy waits.

**The same tiles, the same blocks.** The reference works in tiles of
128 (or narrower for a short picture, §73) with a border of context
it computes and throws away, and every 32-pixel block stops its
iterations when any pixel drops under half its blended start. The
tiling decides the answer at the level of a part in a hundred (the
test in `sharpen.rs` measures it), and the viewport must show what
the export holds, so the GPU does not get to choose a nicer
decomposition: it keeps the reference's. The tiles, padded and
clamped exactly as the reference pads and clamps them, live in an
atlas of two `R32Float` planes (the estimate and the ratio; the
reference's third scratch plane disappears because each blur is
done in one kernel, rows then columns through workgroup memory,
with the reference's sums in the reference's order). A 96 MB atlas
holds about a thousand padded tiles, so a 45 MP frame goes in three
batches and a 24 MP one in two. Per block a `settled` flag and per
tile a count of blocks still going, kept in small storage buffers;
the check after each iteration is a workgroup a block, a minimum
over its pixels, and a block that stops commits its estimate at
once and takes one off its tile's count, so a tile whose blocks are
all done is skipped by every later kernel, which is the reference's
`break`. The automatic contrast threshold, which is RawTherapee's
search for the flattest patch, runs its tile statistics on the GPU
(a workgroup a tile, three rounds, the flattest read back and
decided on the CPU in the reference's own row-major order and tie
rule) and then calls the reference's own `contrast_threshold` on
the one tile it reads back, 25 KB. Since that threshold is a
property of the picture alone, an uploaded `Image` remembers it: a
slider move with the threshold on Auto pays for no search. The
pieces the two paths share (`kernel`, `tile_for`, `border_for`,
`l_star`, `contrast`, `blend_factor`, `tile_index`,
`contrast_threshold`, the tile constants) are public in
greycard-core's sharpen module and marked shared, so that a
difference between the paths is rounding and never a constant.

**Where rounding could show.** Pass by pass: the deconvolution's
blurs, ratio and multiply, the blend blur, the contrast measure and
sigmoid, the clip mask and its dilation, and the final scaling take
the reference's operations in the reference's order, so they differ
from it only by the GPU compiler's freedom to fuse a multiply and an
add. Three places are not order-faithful: L* uses a power and a
Newton step where the reference has `cbrt`; and the tile statistics
behind the automatic threshold, and the row sums behind the stats,
reduce a strided partial per thread and then a tree, where the
reference sums in sequence. A last bit is invisible in a pixel, but
two decisions read it, and there a difference would be a step and
not a rounding: a block's early stop (an estimate a bit under half
its blended start stops an iteration earlier, and that block is
then a whole iteration different) and the threshold search (which
tile is flattest, and the threshold's 0.01 step). Neither has moved
on any picture tried; the tests hold the pictures to 1e-4 and the
threshold to equality, so if either ever does, the suite says so
rather than the viewport.

**How far apart.** The tests (`crates/greycard-gpu/tests/sharpen.rs`)
run both paths on the same picture and print the numbers. On a
synthetic 523x389 picture with a clipped patch, tiles hanging off
its edges, at a fixed radius and threshold: the pictures at most
2.9e-6 apart relative to the reference's value at the pixel, 3e-8
on average, the masks at most 3.3e-6; at radius 1.8 and 30
iterations (the widest kernel and border) 4.0e-6; without the early
stop 3.3e-6; with the measured radius and no threshold 8.4e-7. On
noise at mid grey the automatic threshold comes out the same
hundredth (0.16) and the pictures 2.5e-7 apart; on a busy picture
whose only flat patch the coarse pass cannot see, the fine pass and
the search around its best find the same tile and the same 0.21;
on pictures of 150, 64x48 and 41 pixels, where that search around
the best is the largest of the three (441 tiles, which once overran
the buffer sized for the grids), the same again. On a 2048x1536
region of the R6 II frame `5M0A3976.CR3`, developed by the engine,
under the editor's defaults (the measured radius 0.632, the
automatic threshold, which lands at 0.10 on both): at most 1.5e-6
relative, 1.4e-9 on average, the masks 3.7e-6, and the stats the
same to seven digits. The tolerance the tests hold is 1e-4
relative, twenty-five times the worst measured, left that wide
because another vendor's compiler fuses multiplies and adds
differently and twenty multiplicative iterations amplify what it
does. On the whole frames, `examples/bench.rs`: 24 MP at most
1.4e-6 relative, 45 MP 2.2e-5 (a single specular pixel in the
thousands; the mean is 2.6e-9).

**The §18 check, for real.** The first version of this claimed the
editor's 1:1 screenshot byte-identical between the two paths, and
the review caught that both screenshots had run the CPU sharpen:
the device reached the worker from the rendering setup, after the
first develop was already queued, so every `--screenshot`,
`--snapshot` and `--export` run developed on the CPU. The first
file now opens from the rendering setup, once the worker has the
device, and the log confirms the order (`engine ops on the GPU`,
then `base made, sharpen on the GPU`, then `wrote`). Measured
again on the 24 MP frame, `--no-display-profile`, 940x802 views,
the GPU path against `--cpu-ops`: at 1:1, 19 pixels of 754 thousand
differ, every one by a single step of 255 in one channel, scattered
over the whole view (RMSE 0.031 percent, 0.08 of 255); at fit, 15
pixels the same way; with `--sharpen-mask` painted, 27 at 1:1 and
19 at fit, again by one step. Against the CPU export's crop at 1:1
the GPU view is 0.069 percent RMSE (0.18 of 255) where the CPU view
is 0.066 (0.17): the §18 figure, and the two paths' difference is
under it by a factor of twenty. So not byte-identical: a rounding
flip in one pixel in forty thousand, which is what a last-bit
difference through a half-float texture and an 8-bit encode should
leave.

**How much faster.** The op alone, `examples/bench.rs`, release, on
the 16-core desktop against the RTX 5070 Ti: the 24 MP R6 II frame
217 ms on the CPU, 45 ms on the GPU with the threshold remembered
(61 with the search); the 45 MP R5 II frame 370 ms against 82 (104
with the search). Uploading the picture once is 39 and 65 ms. In
the editor, measured by the new hidden `--time-sharpen N`, which
moves the radius between two values N times after the first
develop and logs each move to the frame that shows it: 24 MP, CPU
path (`--cpu-ops`) 300 to 361 ms a move, mean 327; GPU path 53 to
58 ms, mean 55. 45 MP: CPU 529 to 620, mean 578; GPU 98 to 109,
mean 102. So a sharpen slider is five to six times quicker to its
frame, and the CPU is idle for it, which on a laptop is the
difference between a slider and a wait: §90 put the CPU sharpen at
2 to 4 s on an Air.

**What the worker does.** A base develop keeps, beside the patched
picture, the picture before the sharpen on the GPU (`PreSharpen`):
the local contrast and the dehaze are run on the CPU on a copy as
before, that copy is uploaded, and it is kept while the patched
picture and the Detail section are the same (the `Arc` is held so
its identity cannot be reused). A develop that changes only the
sharpen uploads nothing and runs the op; what comes back to the UI
is `Developed::Texture`, which the renderer takes as its source
where it used to upload halves. The local contrast's timing is
reported only by the develop that ran it; a develop that kept the
picture says "kept" rather than repeating a number from an earlier
one. The device reaches the worker through its queue from the
rendering setup, and the context is built on the worker's thread so
the shaders compile there. Without a device (headless, `--cpu-ops`,
or a picture the device cannot hold) the develop is
`Developed::Halves` as before. The export's `last` picture is not
made on the GPU path, so every export after a GPU develop re-runs
the tail (local contrast, dehaze, sharpen) on the CPU from the
cached base: 0.28 s on the 24 MP frame and about 0.6 s at 45 MP,
on a path that takes seconds anyway, and intended, since the export
is the reference's picture by design.

**Memory, and letting it go.** On the device, at 45 MP: the uploaded
picture 720 MB, four planes 720 MB, the atlas 192 MB, and the output
texture 360 MB (two while the renderer swaps), about 2 GB; at 24 MP
about 1.1 GB. The working planes are kept between runs on a picture
of the same size, since a slider re-run is on the same picture, and
released (`Context::release`) when the sharpen is switched off, when
the file changes, when the lens database arrives and the base is
remade, or when a develop panics; the uploaded picture goes with the
base or with the sharpen's switch. Fine on a 16 GB card; on an 8 GB
unified machine 2 GB is a fifth of everything, and the same budget
question §90 raised for the denoiser's tile applies. Halving the
upload (three planes rather than four channels, or keeping it as
halves and accepting the 1e-3) is the first thing to do if it ever
pinches.

**The CA correction: a different shape, not this one.** The roadmap
line had both ops as "separable blurs", and the CA correction is
not one. Its separable part is the color-shift guard's box blur;
the rest is per-tile votes (weighted quadratic fits with a dozen
neighbor filters, accumulated in single precision in sequence
order), a 3x3 median and a variance gate over the votes, a fourth-
order polynomial fit in double, and a resample per pixel by that
polynomial. Held to the sharpen's standard, the viewport matching
the export to float noise, the votes have to be reproduced closely
enough that no tile flips across the gate, and the fit and the gate
are global decisions in the middle of the pass. That does not make
it a CPU op. Per §56 its 234 ms is dominated by the color-shift
guard, which is a separable blur, and the vote pass, which is
data-parallel; a port of those two with the median, the gate, the
fit and the solve left on the CPU and a small read back between is
the same CPU-decides-between-GPU-stages shape the threshold search
already takes here. So the framing is that it needs its own design
and its own tests, with the reviewer's caveat kept: it is a partial
port with the decisions on the CPU, not a whole-op port, and not an
afternoon at the end of this one.

Also left: the CLI has no `--gpu` for the sharpen (the bench
example is the measurement tool, and the export path is meant to be
the reference's), and the tests that need a GPU say `SKIPPED` on
their output and return when there is no adapter, which `cargo
test` shows only with `--nocapture`; libtest has no better way to
say it.
