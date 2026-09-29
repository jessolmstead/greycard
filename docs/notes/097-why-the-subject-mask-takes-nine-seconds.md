# 97. Why the Subject mask takes nine seconds (2026-09-19)

Asked why the Subject mask runs on the CPU and takes eight or nine
seconds. The provider is by design, and the seconds are mostly not
the model.

**The provider.** `runtime::open` tries CUDA, WebGPU and CPU in that
order and keeps the first that loads the model *and runs it once*.
CUDA is not offered here: the build carries the `webgpu` feature and
the machine has the driver but no CUDA 13 or cuDNN. WebGPU accepts
BiRefNet and dies at `/decoder/Split_33`, as §34 recorded: the
shader binds one input and sixteen outputs, seventeen storage
buffers, and Dawn allows sixteen per stage. Reading the graph with
the onnx package shows the shape of the problem: 66 Splits, of which
50 have sixteen outputs along an axis, a 16 by 16 grid cut in the
decoder. So it is not one unlucky node; it is how the decoder is
written.

**The seconds.** The first mask in a session, release build, this
machine, measured with the ignored `models` test and a throwaway
example:

| WebGPU attempt: upload, run, fail at the Split | 2.5 s |
| CPU session build at Level3 (Level1 is 1.06 s)  | 1.1 s |
| CPU warm-up run on zeros, inside `load`         | 3.2 s |
| The real run at 1024²                           | 2.9 s |

About 9.7 s before the guided refinement, which is the figure seen.
The session is kept in `ai.rs`, so the second Subject mask in the
same run is the 2.9 s alone, and the same shape on the same frame
comes back from the raster cache on disk at no cost. Only the first
one is slow, and two thirds of it is scaffolding.

**Three fixes, in the order they are worth doing.**

1. *Stop paying for the scaffolding.* The warm-up run exists so a
   provider that accepts a graph and then fails on it, which is
   WebGPU's habit, is found out at load; the CPU provider cannot
   fail that way, so the last provider in the list needs no warm-up.
   And the WebGPU failure is a property of the model file and the
   adapter, so it can be remembered beside the model cache, keyed by
   the model's hash and the adapter, and not retried every launch.
   Together: the first mask goes from about ten seconds to about
   four, with no change to the model.
2. *Make WebGPU run it.* Each sixteen-output Split is sixteen Slices
   along the same axis with the same offsets, a mechanical rewrite in
   Python over the fp16 file; nothing else in the graph troubles the
   provider. SAM on the same card embeds in 50 ms where the CPU takes
   650, so the mask would likely fall under a second. The rewritten
   file becomes our own entry in the registry with its hash and the
   MIT notice kept, published beside the denoiser weights.
3. *CUDA.* Install CUDA 13 and cuDNN and build with the `cuda`
   feature. Helps this machine only, and says nothing about the
   Mac or a laptop, so it is the least interesting of the three.

Not done in this session; the roadmap carries the first two.
