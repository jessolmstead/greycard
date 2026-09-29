# 189. The scopes weighed by a mask (2026-09-27)

The Color track's line: a Selection toggle beside the scopes' four
choices, and with it on every bin of the histogram, the waveform, the
parade and the vectorscope counts a pixel by the chosen adjustment's
mask coverage instead of once. A skin mask on the vectorscope shows the
skin's cloud against the skin line alone; a face's mask on the waveform
shows the face's level without the background on top of it.

**Where the coverage comes from.** The analysis draw (§38) already runs
the viewport shader, which already works out every local's weight
(`weights[k]`) to blend its look. So the draw writes the chosen local's
weight into the alpha, which the viewport's target never used: `range.w`
in the params names the local (its index plus one, nothing at zero), and
only `Renderer::analyze` sets it, through `View::weigh_by`. The target is
`Rgba8Unorm`, so the coverage arrives in steps of 1/255. That is the
same precision `gpu_against_cpu` reads the masks at, and it makes the
weighting whole numbers.

**The binning.** `scope.wgsl` takes a `weighted` switch in its mode
(the uniform grows from 16 to 32 bytes, padded). Weighted, a pixel adds
its coverage step `q = round(alpha * 255)` with the same `atomicAdd` in
place of `1u`, and a pixel at zero returns early. Off, it adds `1u` as
before, so the bins are the same to the count. The bins stay `u32`.
The analysis picture is 512 wide with its height following the frame's
shape (768 for a 2:3 portrait), so 255 of each of its 393,216 pixels is
about 100 million at most, far inside a `u32`. The CPU reference is
`scope::weighted_bins`, which shares `accumulate` with `scope::bins`.
Only the weight function differs (1, or `coverage_step`).

**Normalization.** The drawing scales to the bins themselves: the
histogram to its peak, the waveform to its 99.5th-percentile cell, and
the vectorscope to a logarithm of its peak. The absolute counts still
reach the vectorscope through the `+1` in `lit_log`. So the read-back
bins are brought back to pixels with the whole picture's mass, in
`scope::normalize`. A bin holding `n` coverage steps becomes
`n * pixels / sum`, rounded. `sum` is the coverage sum, which is the
histogram's red channel, since every pixel adds its step there exactly
once. Only the clip bins, 0 and 255 of each histogram channel, are
kept at one or more where anything landed, so a pixel at the edge of a
mask still lights a clipping mark. Every other bin rounds. The review
found a floor on every bin wrong: each cell a faint feather pixel
touched came up to 1, which `lit_log` draws at about 27% brightness,
so a soft skin mask painted the background's colors onto the
vectorscope. Weighed by the test's gradient the waveform's mass was
1,061,142 against 1,047,552 unweighted, 2.6% heavy; with the floor on
the clip bins alone it is 1,047,599. The results:
- A small selection's scope has the frame's mass and draws as bright
  as the frame's, not dimmer.
- A mask of one everywhere gives exactly the unweighted bins: every
  step is 255, and `255c * N / 255N` is `c`.
- A mask of nothing gives no bins, with integer arithmetic throughout,
  so there is no division by zero or NaN, and every scope draws empty.
  `normalize` runs on the CPU after the read back, in `analyze`, so the
  GPU and the reference share it.

**What else reads the bins.** All of them are weighed, the histogram
included, because the RGB scope *is* the histogram. So while the
toggle is on, the clipping lamps at the histogram's corners and the
histogram behind the curve editor follow the selection. The clipping
painted over the picture stays whole-frame, since it is drawn per
pixel in the viewport and never reads the bins. The
navigator reads the same analysis picture, so its alpha is set back to
opaque on the read back when the analysis was weighed.

**The toggle.** `scope-selection` on the window, off at launch, never
written to settings.json: a session's toggle. `scope-mask-active` greys
it: it is set each frame to whether the panel's target adjustment exists
and has a non-empty mask, and weighting is asked for only then. An
adjustment whose on switch is off still weighs: the shader takes the
weight before `enabled` applies, as the mask overlay does. That seems
right, since the switch is about the look, not the selection. The
analysis key now carries the weighing local, so switching the toggle,
changing the target or editing the mask brings a new analysis. It sits
right of the Segmented, as a chip in the Segmented's look, and a click
requests a frame. `--scope-selection` opens with it on, beside
`--show-mask` and `--scope`, for a screenshot.

**Measured.** `the_weighted_scopes_are_the_cpus` (GPU, `render.rs`) runs
the field image with three locals: a linear gradient, the skin preset,
and a luminance window above anything the field holds. It draws the
analysis picture as `analyze` does and reads it back, then checks:
- The RGB is identical with the weighing on or off.
- The alpha is within a level of the mask drawn alone (`mask_alone`),
  the same weight.
- The gradient and the skin take in part of the frame, and the high
  window none of it.
- For each scope, weighed by each mask and unweighed, the `analyze`
  bins match the CPU reference over those bytes. Histogram, waveform
  and parade are exact. The vectorscope is 2 of 698,368 apart, both
  unweighed and weighed by the skin. That is a cell edge where the
  shader's float rounds a last bit off the CPU's; it is already there
  without the weighting. The test allows 8 counts.
- A zero mask gives all-zero bins on the GPU.

CPU tests in `scope.rs`: a mask of one everywhere equals `bins` for all
four scopes; a mask of nothing is all zeros, has no clipping, and draws
a histogram of pure ground; the left eighth of a ramp keeps the whole
picture's mass with nothing from the right; a coverage of 51/255 counts
a fifth of a full one; a faint-coverage pixel at a mid level rounds
to nothing in the waveform and on the wheel rather than being lifted
to a count; and a pixel at the lowest step still lands in the clip bin
and lights the mark, while one below the first step does not. Five new
CPU tests and one GPU test.

In the editor, the chip fits beside the picker at the panel's 320 px.

**Left out.**
- Only the chosen adjustment's mask weighs. There is no weighing by a
  single shape of it, and no union of several adjustments.
- The clipping lamps follow the selection while it is on. If they
  should always be the frame's, the histogram's first `HIST` bins would
  need a second, unweighted copy in the buffer.
- The camera's JPEG path returns an alpha of one whatever is asked. The
  scopes are blank while the placeholder is up anyway.
