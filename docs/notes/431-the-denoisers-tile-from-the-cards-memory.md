# §431. The denoiser's tile from the card's memory

*2026-10-26*

A 4 GB laptop card opened the best tier on WebGPU for a 32 MP frame.
The 1024 tile wants 2 to 5 GB (§37; §90 measured 4.2 GB for the best
tier). Dawn logged `vkAllocateMemory failed with
VK_ERROR_OUT_OF_DEVICE_MEMORY` five times, the plausibility check
caught the garbage and fell back to the engine's denoise, and ONNX
Runtime then aborted with `double free or corruption (out)`. Nothing
of ours can catch that abort, so the card must never be asked for
what it has not got.

## Decided

The WebGPU probe now reads the adapter's own memory under wgpu: the
sum of the device-local heaps on Vulkan, `DedicatedVideoMemory` on
DX12 (`runtime::webgpu_memory`). A Mac reports nothing there; its GPU
has the machine's memory, read as §421 reads it; that reading moved
from the editor into greycard-ai so the denoiser takes it itself.

Whenever a denoiser is opened, from the store or from a bare path, by
the editor or the CLI, the tile is chosen for the provider the model
settled on:

- a discrete card with its memory known: 512 under 5.5 GiB (6 GB less
  what a box reports under the sticker), 1024 from there;
- unified memory: 512 under 15 GiB of machine memory (16 GB less a
  GiB, as §421's bounds), 1024 from there, for §90's swap storm;
- memory not known, CPU, or CUDA: 1024, as before;
- Core ML: 1024 always, since its session is fixed to that shape.

`GREYCARD_DENOISE_TILE=<n>` overrides the choice on every provider
but Core ML. One INFO line names what chose: `denoiser tile 512: the
card's 4.0 GiB`, `denoiser tile 1024: 16 GiB unified`, or `denoiser
tile 1024: the card's memory not known`. The editor and the CLI choose
the same way on the same machine, so the disk cache, which keys on
the tiling, serves both. Only the benchmark's explicit tiling is
left as given.

## Measured

The best tier on WebGPU, on one card (RTX 5070 Ti, read as 15.9
GiB), whole frames at two tiles with the 96 margin: a 6000x4000 frame
at 1024 and 512 (2.8 s and 3.3 s), and an 8192x5464 frame at 1024 and
384 (5.4 s and 7.0 s). On that card both pairs are bitwise identical:
the network's reach is inside the margin, so the seams cannot show
and the smaller tile costs time only. Another card or driver may
round differently; the seams still sit inside the margin. The disk
cache keys on the tiling, so a frame cached at one tile is run again
at the other, with the same answer.
`examples/denoise_tiles` repeats the check.

## Limits

- An integrated GPU's device-local heap is the driver's carve-out,
  not what it can reach; the policy reads the machine's memory there.
- On two GPUs the memory is of wgpu's adapter, which may not be
  Dawn's, the same guess the adapter's identity makes.
- 512 on a 4 GB card is inferred from §37's per-tile figures, not run
  on such a card.
- The DX12 read is compiled for Windows on ARM, not run.
