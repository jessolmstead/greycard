# 259. Local contrast on the GPU, as built (2026-10-07)

§251's second item, built on §254's quarter grid. A Texture or Clarity
move used to cost about 0.3 s at 24 MP on the CPU path and now costs
about what a sharpen move does. It was built as designed; this section
records the numbers and what the review changed.

### The op

`greycard-gpu`'s `local_contrast.wgsl` holds eleven kernels, one for
each of the CPU op's passes, in its order:

1. The log luminance is taken once, before either band.
2. Texture's guided filter runs on it, and its gain is kept in a plane.
3. The log is low-passed in place, twice at Texture's radius, for the
   top of Clarity's band.
4. Clarity's guided filter runs on that: exact at full size under
   `COARSE_FROM_RADIUS`, and on §254's grid from it, with the same
   count-weighted block sums, `coarse_radius` and bilinear taps from
   the block centers.
5. One pass applies Texture's gain, then Clarity's. Clarity's clip
   fade reads the brightest channel after Texture's gain.

The constants and the grid's rules are read from core, made `pub`
there, so the two can't drift apart. The box means are separable
passes, each reading its window whole from workgroup memory: 2 KB for
the row pass and 9 KB for the column pass, under the 16 KB default
limit. It needs no adapter feature beyond the defaults the editor
already asks for. We haven't run it on Metal or DX12.

### Agreement

The CPU op is the reference, but on a whole frame its f32 running sums
drift more than the GPU does. The reviewer wrote an f64 port of the op
and held both to it:

| | Against the f64 port, worst relative |
|---|---|
| GPU, NVIDIA and lavapipe | 1.0e-5 to 2.4e-5 |
| CPU | 2.0e-4 to 4.6e-4 |

The CPU's worst pixels sit at the right-hand end of the rows, where
running sums drift. Mirroring the frame moves the CPU's output by up to
4.6e-4 and the GPU's by under 1.6e-5. So the tests hold the GPU to the
f64 port at 5e-5, and to the CPU only within the CPU's own measured
drift.

The sizes tested include 1x1, 2x3, 5x17, 257x33, both sides of the
grid switch (2539 and 2540) and a long edge of 16384, each with clip
off and with clip 1, on NVIDIA and on lavapipe.

The viewport against the export, which stays on the CPU, at Texture and
Clarity +50 on the 24 MP frame:

- sharpen off: RMSE 0.067%, no sample more than one level off;
- sharpen on: 2.2% of samples one level off, and one pixel three
  levels off, at a spot that is already two levels off with no Detail
  on. The sharpen makes that hotspot, and the op adds one level to it.

### The worker

The worker keeps the patched picture on the device (`Base.uploaded`),
keyed on its `Arc`, beside the picture before the sharpen
(`PreSharpen`):

- **A Detail move** runs the op and the sharpen from the kept upload.
- **A sharpen move** reuses the picture before the sharpen.
- **A sharpen-only edit** reuses the upload without uploading again.
- **With the sharpen off**, the op writes the viewport's half-float
  texture itself, with alpha 0.
- **With the dehaze on**, it takes the CPU path, as §251 said, until
  the dehaze has a GPU port.
- **Errors.** A GPU error ends the GPU path for the session, as the
  sharpen's does. `Unsupported` (a box radius over 128, which needs a
  long edge of about 41,000 px) takes the CPU for that develop only.

Each output is a new `Image`, so the sharpen's automatic threshold is
always found on this picture. The status line says "local contrast on
the GPU". It shows no seconds, because submitting doesn't wait for the
GPU.

### Memory: what the review changed

The first version kept every plane it might need, all the time.

- **Too much memory.** At 45 MP with the sharpen and Clarity on, it
  held 4.0 GB on the device, against master's 2.3 GB, and 4.7 GB
  during a move, when two pictures before the sharpen were alive at
  once.
- **A leak.** It also stopped releasing the sharpen's planes when the
  sharpen was turned off with Detail on, which master had done.

Now:

- `Context` has `release_sharpen` and `release_local_contrast`, and
  each path the worker leaves lets go of its own planes.
- A run makes only the planes its sliders need: Texture's gain only
  with Texture, the full-size slope and intercept only for Texture or
  Clarity's exact filter, and the grid only when Clarity takes it.
- The old picture before the sharpen is let go before the new one is
  made.
- The planes are kept between runs only on a discrete GPU. An
  integrated or unified-memory GPU (Apple's) makes them for each run
  and lets them go after. That costs 2 to 3 ms a move at 24 MP and
  about 5 at 45. The reviewer watched the device's memory hold flat
  across twelve runs in both modes.

At 45 MP, sharpen and Clarity on:

| | Master | First version | Now, discrete | Now, integrated |
|---|---|---|---|---|
| Steady | 2.34 GB | 4.01 GB | 3.47 GB | 3.06 GB |
| During a move | | 4.72 GB | 3.47 GB | up to 3.47 GB |

What remains over master on an integrated GPU is the kept upload,
716 MB at 45 MP, which §251 asks for. Uploading again on every move
would cost 39 ms at 24 MP and 65 at 45, most of the gain. If an 8 GB
Mac needs the room, the cheaper cut is to stop keeping the picture
before the sharpen there, so a sharpen move reruns the op from the
upload (11 to 24 ms). That waits on measuring the Mac.

### Timings

`--time-clarity 10` (hidden, beside `--time-sharpen`) moves Clarity
between 0.3 and 0.6 and logs each move to the frame that shows it.
Median, headless, release, one-minute load 3 to 5:

| | GPU | `--cpu-ops` |
|---|---|---|
| 24 MP | 59 ms | 333 ms |
| 45 MP | 118 ms | 651 ms |

A sharpen move at 24 MP is 53 ms, so a Clarity move is now a sharpen
move plus about 6. At 45 MP the sharpen's 90 ms is most of it.

Next on this path: a GPU port of the dehaze, cheap at its
quarter-scale grid, so an edit with the dehaze on gains too.
