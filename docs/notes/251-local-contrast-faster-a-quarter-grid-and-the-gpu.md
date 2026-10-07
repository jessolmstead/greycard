# 251. Local contrast faster: a quarter grid, then the GPU (2026-10-05)

A plan, not yet built: two items on the Speed track. A Texture or
Clarity move is slow to its frame because none of it is on the GPU.
The sharpen is (§118), but the picture the worker keeps there
(`PreSharpen`) is the one after the local contrast and the dehaze,
kept while the Detail section is unchanged, so a Detail move pays for
the whole chain on the CPU: a copy of the patched picture, the op,
the dehaze when it is on, the upload, and the sharpen with its
threshold searched again. At 24 MP, unloaded: about 50 ms for the
copy, 110 for the op at Clarity alone and 170 with Texture (measured
below), 40 to 120 for the dehaze (§104), 39 for the upload and 61 for
the sharpen (§118), so about 0.3 s a move against the 55 ms a sharpen
move takes, and 0.5 to 0.7 s at 45 MP. The op is about a dozen passes
of the whole frame over four planes of it, bound by memory bandwidth,
so it is also the part that stretches most on a loaded machine.

**Measured.** The op alone from the CLI's `-v` line, release, 32
threads, five runs each, the one-minute load 2 to 5 at the start:
the 6000x4000 R6 II frame 0.11 s at Clarity +0.5 every run and 0.17
with Texture +0.5 beside it; the 8192x5464 R5 II frame 0.21 to 0.26
and 0.38 to 0.45. The box mean is running sums, the same cost at any
radius, so Clarity's guided filter at 150 pixels costs what Texture's
at three does, about 0.06 s at 24 MP; the rest of Clarity's 0.11 is
the log, the low-pass and the apply.

**First, Clarity's coarse filter on a quarter grid.** The guided
filter at Clarity's radius (150 pixels at 24 MP) is half the op at
Clarity alone. He and Sun's fast guided filter (arXiv 1505.00996) takes the
coefficients at a reduced scale and applies them at full size:
reduce the input by a step `s`, run the guided filter's first three
stages there (the means, the slope and intercept, their means over
the windows) at a radius of `r / s`, upsample the averaged slope and
intercept bilinearly, and take `slope * I + intercept` with the
full-size `I`. At `s = 4` the coarse filter costs about a sixteenth
of what it does, which is about half the op at Clarity alone: 0.11 s
to about 0.06 at 24 MP, so a move in the editor goes from about 0.3 s
to 0.25 on the desktop. The gain is the CPU's, so it is larger where
the CPU is the wait (the Mac, a loaded machine, the export); on the
desktop the move's time is mostly the copy, the upload and the
sharpen, and only the GPU port takes those away. Texture's filter at
three or four pixels stays at full size, and so do the low-pass that
makes Clarity's band's top, the mid-tone weight and the clip fade,
all of which read the full-size base.

What the implementer needs to hold to:

- Reduce `I` and `I²` both, by the mean over each `s` by `s` block,
  and take the variance from those. The paper's version reduces `I`
  and squares it at the reduced scale, which drops the variance of
  the structure finer than a block from every window: a textured
  window would look flatter than it is, its slope smaller, and
  Clarity would lift it more than it does now. With the squares
  reduced by block means, each window's mean and mean square are the
  full-size ones to within the window's edge snapped to the block
  grid.
- The window: `r / s` rounded, so `(2r' + 1) s` pixels across against
  `2r + 1` (300 against 301 at 24 MP). A last block that runs off
  the picture is the mean of what is inside, as the box mean already
  cuts its window at the edge; the bilinear upsample puts a block's
  coefficient at the block's center and clamps at the edges.
- `s` is 4 when Clarity's radius is 64 or more (a long edge of 2560
  and over) and 1 below it, the exact filter, so a small picture and
  `MIN_CLARITY_RADIUS` keep what they have. The exact path stays
  bit for bit today's: add the op to core's `ops_alone` harness
  (`develop/timing.rs`, which has no local contrast yet), whose
  digest of the output's bits shows that, and whose times are the
  before and after. Every test in
  `local_contrast.rs` today is a small picture and would take the
  exact path, so the fast path needs a way to be forced (an internal
  function taking `s`) and tests of its own: against the exact
  filter on a picture large enough to take it, the base within a
  small tolerance in stops away from the edges; and §103's behaviors
  run again through it, the soft bump's lift at ±1, the four-stop
  edge's overshoot under a third of a stop, the period-six grating
  within five percent, the mid-tone fade, the clip guard.
- `LocalContrastStats` still reports the full-size radius, so the
  status line keeps saying "3 and 150 px".
- Seen and measured again as §103 was: the soft bump's lift and the
  four-stop edge's halo from its epsilon table at the chosen 0.25,
  the grey standard deviation of the altar and the face crops at
  Clarity +1 and -1, and the editor's viewport at +50 against the
  exact filter's. A difference anyone can see is a reason to stop
  and look, not to tune the gain.

**Then, local contrast on the GPU.** With the coarse filter on a
quarter grid, nothing in the op is a large box at full size: the log,
Texture's filter at a radius of three or four, the two box passes of
the low-pass at the same radius, the block reduction, the coarse
filter on a plane of 1500x1000 at 24 MP, and the full-size apply.
Those are small separable passes and per-pixel arithmetic, which the
GPU does well. The port is why the quarter grid goes first: a
150-pixel box over a full-size plane is the awkward part on a GPU.

- The CPU op is the reference, and the GPU's is tested against it
  the way the sharpen's and the CA correction's are: a
  `tests/local_contrast.rs` in greycard-gpu on the pattern of
  `tests/sharpen.rs` (its `context()` that prints SKIPPED without an
  adapter and fails under `GREYCARD_REQUIRE_GPU`), the pictures held
  to 1e-4 as the sharpen's are, on lavapipe with
  `GREYCARD_REQUIRE_GPU` set as well as on the desktop's GPU (§221),
  with Texture and Clarity alone and together and a clip level
  given, and on a picture large enough to take the quarter grid.
  Agreement is to rounding, not to the bit: `log2` and `exp2` differ
  by device, and sums are taken in a different order (the CPU's
  running sums drift in f32 where a GPU reading its window whole
  does not). The op goes into `examples/bench.rs` beside the
  sharpen.
- The order to keep: the log is taken once, from the picture before
  either band; Texture's gain is applied first; Clarity's band is cut
  from that same original log (not one taken again after Texture);
  Clarity's clip fade reads the pixel's brightest channel after
  Texture's gain. The two gains compose.
- Box sums separable, one pass along each axis, never a 2D
  summed-area table in f32: over a whole frame its totals reach
  billions and lose the precision a variance against an epsilon of
  0.25 needs. At these radii the passes can read their window
  directly from workgroup memory.
- A GPU op's output has to be an `Image` the sharpen takes, and an
  `Image` is only made by `Context::upload` today, its texture
  `TEXTURE_BINDING | COPY_DST`; the op's output wants
  `STORAGE_BINDING` as well. And an `Image` holds the sharpen's
  automatic threshold in a `OnceLock`, found once as a property of
  the picture: every Detail move is a new picture and must be a new
  `Image` (or one whose threshold is cleared), or the sharpen keeps
  the last picture's threshold and nothing says so.
- The worker keeps the patched picture on the device, keyed on the
  patched `Arc` alone, and the picture after the local contrast
  beside it, keyed on the Detail section as `PreSharpen` is now: a
  Detail move runs the op and the sharpen on the GPU from the kept
  upload, and a sharpen move still reuses the picture after the op.
  An edit with the dehaze on takes today's path, the op and the
  dehaze on the CPU and an upload, until the dehaze has a GPU port of
  its own; that is a later item, cheap at its quarter-scale grid. A
  move that turns the dehaze on or off changes path, and the kept
  textures of the path left are let go.
- With the sharpen off the worker has no GPU path today: the
  `(Some(ctx), None)` arm of `develop_job` releases the device's
  textures and develops on the CPU, and the viewport gets `Halves`.
  The op on the GPU should then write the viewport's texture itself
  (`Rgba16Float`, as `sharpen_apply.wgsl` writes it, the alpha the
  sharpen's mask, so zero, as `Halves::from_image` writes it with no
  mask), or an edit with the sharpen off gains nothing from the
  port. `Context::release` releases the sharpen's textures only, and
  has to take the op's too.
- Memory: the patched picture in `Rgba32Float` is 384 MB at 24 MP
  and 720 at 45, and the op's planes in `R32Float` about 96 and 180
  MB each, beside the sharpen's. An 8 GB Mac shares that with
  everything else, so the planes are made for a run and let go, or
  kept only while there is room; `Context::fits` still bounds the
  side.
- Failure is as the sharpen's: the first GPU error logs, releases the
  device, and every develop after is the CPU's for the session. The
  export stays on the CPU path, as it does for the sharpen.
- The status line says "local contrast on the GPU" as it says
  "sharpen on the GPU". The measure is the editor's, a hidden flag
  like `--time-sharpen` that moves Clarity between two values N times
  and logs each move to the frame that shows it; the aim is about a
  sharpen move's time, at 24 and at 45 MP, with the CPU path beside
  it.
