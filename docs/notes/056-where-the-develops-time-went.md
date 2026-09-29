# 56. Where the develop's time went (2026-09-18)

Profiled stage by stage on the 45 MP R5 Mark II frame (8480x5650),
default edit, release build, on the 16-core desktop: prepare 1034 ms,
of which the CA correction was 730 and the noise estimate 164; the
dual demosaic 841 (AMaZE 300, VNG4 394, the rest the blend); finish
62; the sharpen 1239; and the conversion to half floats for the GPU
230. Four of those were structure, not algorithm, and are fixed:

- **The sharpen's tile blur** clamped the index on every tap, which
  kept the compiler from vectorizing the one loop the whole
  deconvolution lives in. The interior of a row now runs unclamped
  over windows, the edges as before, and the column pass is a weighted
  sum of whole rows. 1239 to 610 ms. The tests hold; the sums run in a
  different order, nothing else.
- **The CA correction's color-shift guard** was 555 of its 730 ms:
  a serial pass over the frame for the factors, and a column box blur
  that gathered each column into its own vector and scattered it back
  across a 12 MP plane. The factors are made a row pair at a time in
  parallel, and the blur runs its three row passes, transposes, runs
  three more, and transposes back, the passes commuting. 736 to 234 ms.
- **The noise estimate** was one thread over the mosaic; it runs on
  every base develop because the dual demosaic's automatic threshold
  wants it. Block rows in parallel, each with its own bins, merged in
  order so the result is the same. 164 to 20 ms.
- **The half-float conversion** in the worker was a serial push loop.
  A row at a time in parallel: 230 to 35 ms.

A develop is 2.0 s from 3.4 s; a sharpen slider change, which redoes
only the sharpen and the conversion, is 0.65 s from 1.5. What was
tried and did not pay: `-C target-cpu=native` (the half conversion
takes F16C and drops to 55 ms, the demosaic loses 120 ms, the sharpen
gets slower), fat LTO with one codegen unit (nothing), and the
export's Lanczos resize (291 ms to 2048 wide; fine). What is left is
in the roadmap: the dual demosaic makes the VNG4 half under every
pixel when a third is blended away here, the profiled denoiser is
7 s on this frame, and the sharpen's 32-pixel tiles with a 5-pixel
border compute 1.7 pixels for every one they keep.
