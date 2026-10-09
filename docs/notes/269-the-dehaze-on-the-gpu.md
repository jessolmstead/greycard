# 269. The dehaze on the GPU (2026-10-08)

§259 left one gap in the viewport's GPU path: with the Dehaze on, the
local contrast and the dehaze both ran on the CPU and the result was
uploaded, so an edit with the Dehaze on paid about 170 ms for a Dehaze
move and 260 ms for a Texture move at 24 MP, where a Clarity move
without it costs 62. The dehaze now has a GPU port of the half that
scales with the picture, and the local contrast and the sharpen stay on
the GPU with it. The export is still the CPU reference, unchanged to
the bit.

### The split

The dehaze (§104) was already two halves:

1. **The fit**, on a reduced copy of the picture, a long edge of about
   1536, each reduced pixel the mean of its block (four by four at
   24 MP): the dark channel, the airlight from two quantiles, the
   guided filter's slope and intercept from box means. Small (1.5 M
   reduced pixels at 24 MP), and the two selections are awkward on a
   GPU. It stays on the CPU, the reference's own code.
2. **The apply**, per full-size pixel: the line read bilinearly off the
   grid and closed by the pixel's own luminance, bounded below by the
   pixel's own dark channel, then `J = (I - A) / t + A`. This is what a
   24 or 45 MP picture pays for, and it is the part we ported.

So a GPU run is: the block sums on the GPU, read back (18 MB at 24 MP),
divided on the CPU into the reference's reduced copy, the reference's
fit on that, the model sent back (its two grid planes and the bilinear
taps of every column and row), the apply on the GPU, then the sharpen
as before.

In core, `fit()` became a type, `Reduced`: the block means and what is
read off them that the amount does not change (the luminance, the
airlight, the guide's two box means). `Reduced::model(amount)` does the
rest of the fit, and `Model` is what the apply reads. `Reduced::new`
makes the copy from a picture, as the CPU always has;
`Reduced::from_block_sums` makes it from sums the GPU read back. The
CPU path is `Reduced::new(image).model(amount)` then the apply, the
same arithmetic in the same order as before.

### Making the reduced copy the reference's to the bit

The airlight is the mean of the pixels past two quantiles, so a
reduced copy a rounding away from the reference's can pick other
pixels. We avoided the question rather than measuring around it: on
the same input pixels the GPU's copy is the reference's to the bit.

- The block sums are added the way the reference adds them, from zero,
  along each row of the block and the rows in order, one invocation a
  block. Vulkan requires a float add to be correctly rounded, so with
  the order fixed nothing is left to differ.
- The division by the block's count, which on a GPU is good to 2.5
  ulps and not to rounding, is done on the CPU by the reference's own
  code (`divide`).
- The bilinear taps of every column and row are the reference's `taps`,
  computed on the CPU and uploaded, so the apply reads the grid at
  exactly the reference's weights.

The fit then makes the same model on both paths, slope and intercept
to the bit, and the paths differ only in the apply's per-pixel
arithmetic. On NVIDIA (RTX 5070 Ti) and lavapipe the sweep holds the
block means equal and the models equal bit for bit in every case.

This holds where the device adds as Vulkan says. Metal compiles with
fast math on (wgpu does not turn it off), which allows reassociating a
chain of adds, so on a Mac the copy may differ from the reference's by
the reassociation's reach. The tests hold it to that reach there and
check the rest on the GPU's own model. We have not run it on Metal or
DX12.

### What the local contrast before it does to the fit

With Texture or Clarity on, the dehaze reads the local contrast's
output, which on the GPU differs from the CPU's by §259's rounding (the
CPU's own running sums drift more than the GPU does). That difference
reaches the airlight's quantiles. We measured it in the sweep, running
the fit on the CPU's local contrast output and on the GPU's for every
case that draws a local contrast:

- Default seed, 60 cases (25 with a local contrast, 21 of them with a
  dehaze to fit): the airlight moved 8.6e-5 at most, the mean
  transmission 5.5e-6.
- Seed 1, 1000 cases, and 0xdeadbeef, 600: up to 1.3e-2 on the
  airlight and 6.8e-3 on the mean transmission (5.6e-3 and 1.9e-3 on
  the second seed), on synthetic scenes built to break it: blocks of
  pure primaries and flat steps, where thousands of pixels tie at the
  quantile and a rounding decides which side they fall on, under
  Texture or Clarity at the slider's end.
- On the five real frames below, the airlight and the mean
  transmission agree to the 1e-5 they are printed to in every case.

So on real pictures the fit is the same on both paths; on a picture
made of a few exact colors it can move by a percent or two of the
airlight (2.1%, 0.148 on an airlight of 7.07, in one case of the
review's seed). We record it here rather than hold it in a test, since the
local contrast's own sweep holds its rounding.

### The apply and its tolerance

The apply is one kernel, a pixel an invocation, with the reference's
order of operations. Its result agrees with the reference to rounding,
not to the bit: the GPU's divisions are good to 2.5 ulps and a device
may fuse a multiply and an add.

The tests hold it to an f64 evaluation of the same f32 model and taps,
by a bound counted per sample, operation by operation, from that
sample's own magnitudes, at the precisions Vulkan requires: half an
ulp for an add, a subtraction or a multiply, 2.5 ulps for a division.
A bilinear read of a plane comes to ten half-ulps of its magnitude,
the transmission's line to `17 e |slope| L + 11 e |intercept| + 8 e`
(the slope's term and the intercept's counted separately, since a
steep slope and its intercept can cancel; `L` the luminance's
magnitude), and the recovery to `e (6 (|I| + |A|) / t + 2 |J|)` plus
the transmission's error carried through the division, `|I - A| / t^2`
times it. A fused multiply and add rounds once where the count has two.
The CPU's own f32 apply must meet the bound too. Both paths are held to
one times it:

| | Worst, as a share of the bound |
|---|---|
| GPU, NVIDIA, 1660 cases | 0.172 |
| GPU, lavapipe, 1060 cases | 0.180 |
| CPU, the same cases | 0.180 |

On lavapipe the worst sample of every case is the CPU's, at the same
share of the bound, which is to say lavapipe rounds as the CPU does;
NVIDIA, which fuses, lands elsewhere inside it. A shader that read a
tap, a weight or a channel wrongly would be thousands of times over.

The stats keep their meaning: the transmission's mean and least over
every full-size pixel, after the bound and the floor. The apply sums
and takes the least of its transmissions a workgroup at a time, the
pairs are read back and summed in f64, and both are held within 1e-5
of the reference's on the same model. The mean is what the status line
shows ("dehazed +50 to a mean transmission of 0.92"); the least is the
CLI's.

### The sweep

`greycard-gpu/tests/dehaze.rs`, in §264's manner
(`GREYCARD_OP_PARITY_SEED`, `_CASES`, `_ONLY`), 60 cases by default, a
few seconds in debug:

- **The amount** over -1 to 1, each end a fifth of the time, zero a
  tenth, small values three twentieths: haze taken out and put in.
- **The local contrast** on the GPU before it in two cases of five,
  Texture and Clarity drawn the same way, the clip level or none, as
  the viewport runs them.
- **Sizes** at factors one to eleven: small pictures from 1x1, a long
  edge from 1530 to 6200, a long edge exactly at or one past a
  multiple of its factor, widths 105 to 111 past a multiple of 112
  (§264's CA tiles; a picture reaching the dehaze has been through
  them), and long edges from 8000 to 16384 on a short side.
- **Scenes**: hazy under a drawn airlight (through and under the
  neutrality floor), clear, neutral, steps of a few stops, noise from
  deep shadow to past white, flat (black, 1e-7, mid grey, white,
  clipped at 8), pure primaries, and hazy with clipped patches.

Each case checks the block means (to the bit on Vulkan), the model (to
the bit on Vulkan), the apply against the bound, the stats, and on
Vulkan the whole op (`Context::dehaze_image`) against the reference's
`dehaze` on the CPU. Fixed tests beside it: a hazy picture at four
amounts and two factors, the edge sizes (1x1, 2x3, 5x17, 1536 and 1537
long, 9x3073, 4609x5, 16384 long both ways), zero as the identity, the
viewport's half floats against the full floats with alpha zero, and a
device error caught as an error. Seeds run: the default and seed 1
(1000 cases) on NVIDIA and lavapipe, and 0xdeadbeef (600 cases) on
NVIDIA; none failed.

Fifteen hand mutations of the shader: a luminance weight, a tap index,
the row weight, the bound dropped, the airlight's channels swapped, the
cap on haze put in, the recovery off by 1e-5, the block sums' order
(rows and columns swapped), the block's edge, a sum's channel, the
stats' reduction over half a workgroup, the stats' least taken as a
greatest, a column's tap from the row, and the strength 1% off. The
sweep and the fixed tests catch fourteen. The fifteenth, the floor of
0.2 lowered to 0.19, cannot be caught: the bound `1 - s d` is at least
`1 - 0.8 = 0.2` already, so the floor never binds at the slider's
range, on either path. The block sums' order is caught only because
the check there is to the bit.

The CPU side has its own record:
`the_output_is_what_it_was_bit_for_bit` holds the CPU dehaze's output
bits, airlight and least transmission on eleven synthetic pictures
(factors one to four, sides not a multiple of the factor, black and
clipped frames) to what the code gave before the split, recorded in
its own commit before the refactor. `the_block_sums_divide_as_the_reduction_does`
holds sums made one block at a time, in the GPU's order, to the CPU's
reduced copy and model bit for bit.

In the editor's tests: the Detail section on the GPU against the CPU's
picture (local contrast then dehaze, within two half-float ulps);
the cache's paths and what each keeps on the device; a Dehaze move
from the kept reduced copy against a develop made afresh at that
amount, the viewport's texture equal to the bit with the sharpen off
and on; a GPU error in the dehaze taking the session to the CPU, its
develop the CPU's to the bit; and a picture larger than a device that
takes 256 a side, with a Detail slider set and the sharpen off and on,
the CPU's for that develop with the context kept, and the next picture
that fits the GPU's.

### The worker

`detail_on_gpu` runs the Detail section on the device from the
patched picture's kept upload: the local contrast when a slider of it
is set, then the dehaze. With the sharpen on, the result is the
picture before the sharpen, kept as before. With the sharpen off, the
last op writes the viewport's texture.

- **A Dehaze move** runs the local contrast again from the upload,
  then fits from the kept reduced copy (`KeptHaze`, keyed on the
  patched picture's `Arc` and the local contrast's options): no block
  sums and no read back. The GPU's local contrast gives the same
  picture from the same inputs, so the kept copy is that picture's.
  We chose this over keeping the local contrast's output, which would
  hold another full-float picture on the device (716 MB at 45 MP) to
  save its 6 to 12 ms.
- **A Texture or Clarity move** with the Dehaze on makes the copy again.
- **A sharpen move** keeps the picture before the sharpen, Dehaze and
  all, as before.
- **Errors.** A GPU error (a validation failure, out of memory, a lost
  device) is warned once and ends the GPU path for the session, as
  before. A picture larger than the device's textures (16384 a side;
  in practice a large stitched TIFF) is `Unsupported`, from the upload
  or an op, and takes the CPU for that develop only, with the context
  kept for the next picture, with the sharpen on or off. The viewport's
  texture is made only once the upload has found the picture fits: the
  first cut made it before, outside any error scope, and a picture past
  the limit raised a validation error in wgpu's uncaptured handler,
  which panics the worker thread; the review found it. On the sharpen's
  path, the code before also took an oversized picture's `Unsupported` for a
  device error and ended the session's GPU path; that is now the
  CPU for that develop too. An amount so small that its strength rounds
  to zero also takes the CPU path, which leaves the picture alone, as
  the CPU path always has.
- **Memory.** The steady state is what the local contrast path already
  holds: the upload and the picture before the sharpen, plus the kept
  reduced copy on the CPU, about 36 MB at 24 MP. During a move with
  Texture or Clarity, the Dehaze and the sharpen all on, three
  full-float pictures are alive at once: the upload, the local
  contrast's output and the dehaze's, one more than the path with the
  Dehaze off, for the length of the move. A full-float picture is 384
  MB at 24 MP, 716 MB at 45 MP and 1.63 GB at 102 MP. Neither can go
  early without a cost: the apply reads the local contrast's output,
  and the upload is what the next move starts from (letting it go
  would mean uploading again on every move, 39 ms at 24 MP and 65 at
  45). Writing the dehaze into the local contrast's output in place
  would need read-write access to an `rgba32float` storage texture,
  which is not portable. With the sharpen off the dehaze writes the
  viewport's half floats and there is no third picture. The sums, the
  model and the stats are a few tens of MB, made for each run. Every
  one of these is made inside the op's error scope (the upload's, the
  local contrast's, the dehaze's), so a device out of memory there is
  caught as an error and takes the session to the CPU rather than
  panicking; the viewport's half-float texture is made outside one, as
  before. Against the old CPU path for the Dehaze, the device now
  also holds the upload, as §259 already does for the local
  contrast.
- **The log** says "dehaze on the GPU".

There are two waits on the device in a move: the block sums are read
back before the fit (on a move that makes the copy), and the stats are
read back after the apply, before the sharpen is submitted.

The reference's fit itself got faster on the way, without changing a
bit: its element-wise loops run on rayon, and the box mean's column
pass runs in bands of 64 columns, each band with its own running sums,
added down each column in the same order as before. At 24 MP the fit
at an amount went from 12.0 to 7.8 ms and the reduced copy from the
picture from 14.9 to 12.6 ms. The CPU's whole dehaze, the export's,
went from 37.7 to 31.6 ms (least of 15, `ops_alone`, one-minute load
about 5).

### Timings

The hidden `--time-dehaze N` and `--time-texture N` (beside
`--time-clarity`) move Dehaze between +30 and +60, and Texture
between +30 and +60 with Dehaze held at +50, and log each move to the
frame that shows it. The editor ran headless under weston
(`--backend=headless --renderer=gl`), release builds, the sharpen on
at the default, 16 moves each, the median. Before is the tree before this change
with only the timing flags added, built beside it; the two were run
alternately. One-minute load 1.3 to 3 for these, except 7 for the
45 MP Texture move after:

| Move | 24 MP before | 24 MP after | 45 MP before | 45 MP after |
|---|---|---|---|---|
| Dehaze | 170 ms | 72 ms | 293 ms | 138 ms |
| Texture, Dehaze +50 | 258 ms | 103 ms | 502 ms | 177 ms |
| Clarity, no Dehaze | 62 ms | 63 ms | 127 ms | 127 ms |
| Sharpen, no Detail | 55 ms | 55 ms | 104 ms | 105 ms |

A second round under another build's load (one-minute load 9 to 22)
gave 171 to 74 and 295 to 141 for the Dehaze move, 259 to 99 and 495
to 196 for the Texture move. A Dehaze move is now a sharpen move plus
about 17 ms at 24 MP and 34 at 45; a Texture move with the Dehaze on,
a sharpen move plus 47 and 73. The Clarity and sharpen moves are where
they were.

### The viewport against the export, real frames

The ignored test `the_dehaze_viewport_against_the_export_on_real_frames`
develops each frame twice from one base made on the CPU (so the CA
correction is the same): the export's picture on the CPU, the
viewport's on the GPU. Both are finished the export's way (the edit's
look, sRGB, eight bits) and compared sample by sample. Five frames: a
45 MP landscape, a 24 MP interior with a bright window, the 24 MP dusk
vista of §104, a 102 MP frame, and a 24 MP studio portrait. Four
Detail settings: Dehaze +50; Dehaze +50 with Texture and Clarity +50;
Dehaze -50 with Clarity +30; and Dehaze +100 with the sharpen off.

In all twenty cases, before and after: no sample is two levels or more
off, the 99.9th percentile is one level, and the worst is one level.
The share of samples one level off, before and then after:

| | +50 | +50, Texture and Clarity +50 | -50, Clarity +30 | +100, sharpen off |
|---|---|---|---|---|
| 45 MP landscape | 2.20, 2.20% | 2.17, 2.17% | 2.31, 2.31% | 1.15, 1.80% |
| 24 MP interior | 1.79, 1.79% | 1.77, 1.77% | 1.84, 1.84% | 1.06, 1.66% |
| 24 MP vista | 1.71, 1.71% | 1.71, 1.73% | 1.90, 1.90% | 0.76, 1.23% |
| 102 MP | 1.86, 1.86% | 1.86, 1.86% | 1.95, 1.95% | 0.66, 1.11% |
| 24 MP portrait | 2.22, 2.22% | 2.18, 2.18% | 2.45, 2.45% | 1.04, 1.63% |

With the sharpen on, the difference is the sharpen's, as before: the
dehaze adds nothing measurable to it. With the sharpen off, the
viewport used to be the CPU's own picture in half floats and is now
the GPU's, and the share one level off rises by about half a point. We
traced that to the half floats, not to the dehaze. The GPU's store to
an `rgba16float` texture rounds toward zero on both NVIDIA and
lavapipe: 56% of samples are not at the nearest half float, where the
CPU's `Halves` round to nearest. That is true of every GPU op that
writes the viewport's texture (the sharpen and the local contrast do
it today), and is why the sharpen-on cases sit at about 2%.

### The sharpen's dots with the Dehaze on

The dehaze pushes noise at black further below zero, which is what
§268's pedestal is for, so the real-frame measurement also counts §268's
dots (a pixel whose brightest channel is over 0.8 in a 16-bit finish
while the 5x5 mean of its Rec.709 luma is under 0.08), on the export's
picture and the viewport's. On the ISO 10000 frame of §268, with the
sharpen on: none on either path at Dehaze +50, +60, +50 with Texture
and Clarity +50, and -50 with Clarity +30. The same count on a build
from before §268 gives 1,352, 1,387, 1,283 and 5, the same on both
paths, the 1,387 at +60 being §268's own number, so the count finds
what it is looking for. The sharpen's own sweep passes on NVIDIA and
lavapipe on this branch.

### Open

- **Metal and DX12** are not run. On Metal the block sums may
  reassociate under fast math, and the tests allow for it there; the
  airlight could then differ from the export's by a quantile's
  sensitivity, as with the local contrast above.
- **Rounding toward zero in the viewport's half floats.** It is a
  bias of up to one half-float ulp (2^-10, about 0.1%) toward zero on
  every GPU output, a level in some places. Rounding to nearest in
  the shader before the store would remove it for all the ops. It is
  its own change.
- **Synthetic pictures of a few exact colors** can move the airlight by
  a percent or two between the viewport and the export when Texture or
  Clarity is at its end, through ties at the quantile. We found
  nothing of the kind on real frames.
- The fit is still about 8 ms of CPU on every Dehaze move at 24 MP, and
  the two waits on the device sit in the move.
- **An oversized picture with the sharpen on runs the CPU's Detail
  section twice a develop.** When the upload declines,
  `make_pre_sharpen` makes the picture before the sharpen on the CPU,
  finds the upload declines it again, and the develop falls to the
  CPU's path, which makes it once more. Only a picture past 16384 a
  side, and only time; declining as soon as the upload does would
  save it.
