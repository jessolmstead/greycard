# The lens correction on the GPU (2026-10-21)

After the demosaic, the lens correction was the largest stage of a
fresh base (0.7 s at 45 MP on eight threads, §378). It is a resample
with no decision in it, so it moved first. `develop::lens::correct`
stays the reference, the export's path, the `--cpu-ops` and no-GPU
path, and the fallback.

**The op.** `Context::correct_lens` in greycard-gpu, a thread a pixel:
each channel read at its own place when there is a CA model, else at
the green's; Catmull-Rom over four by four texels clamped to the
picture, zero off it; the vignetting's gain taken where it read. The
magnification and the corner gain are the reference's functions on the
CPU; the models are floats in a uniform, every expression in the
reference's order. Full floats up and back, since the result is the
base the worker keeps; the output is read back in bands of about
64 MB through two staging buffers, so a run holds the input and two
bands, not a second frame.

**Division.** NVIDIA uses Vulkan's 2.5 ulp of room in `/` and `sqrt`;
at a position near 8000 pixels that is visible in the fourth decimal
across a sharp edge. The shader corrects each quotient and root once
from the residual `fma` gives. On a real 45 MP frame the median
difference is then zero, the 99.99th percentile 5.9e-5, the worst
1.7e-3; what remains is the device's freedom to fuse, which WGSL
cannot forbid.

**The worker.** `lens_step`, shared by the develop and the prefetch,
runs a correction that moves the picture on the GPU; vignetting alone
stays on the CPU, a multiply a pixel being quicker than the trip. A
size the device cannot hold falls back for that base; a device error
falls back and drops the context for the session, as after the CA's.
The base's `ca_on_gpu` became `on_gpu`, so the export, which never
takes a GPU base, makes its own.

**Tolerance.** A position lands a unit or two in the last place from
the reference's, and a unit grows with the coordinate; Catmull-Rom's
slope across the test's steps is at most 0.56 a pixel. So the bound is
0.56 times four units of the longer side, floored at 1e-4: 1e-4 at
301 pixels, 2.2e-3 at 8192. Vignetting alone holds to 2e-6; the
identity gives the input back exactly.

**Measured.** 45 MP, three fresh opens each:

| | lens | fresh base |
|---|---|---|
| eight threads, before | 0.82 s | 2.61 s |
| eight threads, after | 0.15 s | 1.92 s |
| 32 threads, before | 0.29 s | 1.48 s |
| 32 threads, after | 0.18 s | 1.39 s |

It pays on eight threads or fewer and is near neutral on a many-core
desktop, where the time is mostly the 720 MB each way. Integrated,
AMD, and Apple GPUs are unmeasured. The viewport before and after is
identical to the eye: under one pixel in a pair differs, by one 8-bit
level.

**Left.** Handing the lens output to the Detail section's upload
rather than uploading it again would save about 0.09 s more.
