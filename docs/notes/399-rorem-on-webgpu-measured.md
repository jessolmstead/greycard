# §399. RORem on WebGPU, measured

*2026-10-23*

§380 chose RORem as the open model trained on removal pairs, with
LaMa kept for the CPU, and named the first check: whether the WebGPU
provider loads a 5 GB UNet under Dawn. It does, and the result is
good enough to ship. The findings, with the corrections to §380.

## Corrections to §380

- **The license is not Apache.** The model card carries a bare
  `apache-2.0` tag and the repository's LICENSE link is dead, but the
  weights are a full fine-tune of SDXL inpainting, so CreativeML Open
  RAIL++-M carries through with its use restrictions. Fetched as data
  on first use it fits §36's reading if the fetch shows the RAIL
  text; a mirror on our account must ship the license. Two training
  sets (MuLAn, CC BY-NC-SA; DIV2K, academic) may or may not bind the
  weights; legally unsettled, and the user's call.
- **RORem-mixed, trained at 512 and 1024**, is the published model;
  the plain RORem is 512 only.
- **No prompt and no guidance.** It was trained on the empty string at
  guidance 1.0, so the text encoders run once for constant embeddings
  and each step is half the usual SDXL figure.
- **No CUDA here.** Our packages ship pyke's WebGPU-only ONNX Runtime;
  WebGPU through Dawn is the first and only provider.

## What runs

The fp16 UNet (5.1 GB, external data beside a 2.5 MB graph), exported
by a fp32 trace at opset 17 then converted, loads whole under Dawn.
Twenty-five steps at 1024 take 9.0 s on an RTX 5070 Ti, 0.36 s a
step, with the UNet at 6.8 GB; CUDA does the same in 3.3 s. Two
export corrections: the fp16 converter makes Resize's `scales` input
fp16, which is invalid ONNX, and WebGPU's `Conv` gives a wrong answer
for any input channel count that is a multiple of 3 but not of 12 (6,
9, 15, 18), in 1.27 and 1.28, fp16 and fp32. The UNet's 9-channel
`conv_in` is padded to 12 zero channels, which fixes it exactly; a
reproducer is kept for an upstream report. The pinned build has no
WebGPU GroupNorm kernels, which does not matter at opset 17, where
GroupNorm becomes InstanceNormalization, but an Olive-fused graph
must never be fed to it.

RORem's own VAE is the fp16-fix one already (247 of 248 tensors agree
with it; 6 with the stock SDXL VAE), so it decodes in fp16 with no
overflow.

## Is fp16 on WebGPU good enough

fp16 drift on WebGPU is about 18 times CUDA's, 1.9 percent at the
UNet's output after one step. A whole removal says it does not show:
two people on shore rocks, a 1024 window, a hand-drawn mask covering a
quarter of it, 24 UNet calls at strength 0.99, seed fixed. Against the
torch fp32 reference the hole's PSNR is 38.8 dB (SSIM 0.979) with
everything in fp16; CUDA fp16 gives 44.8. Side by side at 2x the two
cannot be told apart; the difference map is glints and grain with no
structure; the one shift is a uniform 0.8/255 darkening from the fp16
VAE. The fill is clean, rock and water carried through, no ghost.

## Time and memory, the whole pipeline

| At 1024 | Time | Peak over the desktop |
|---|---|---|
| encoder, UNet, decoder resident | 11.0 s (loads 8.4 s on top) | 12.2 GB |
| one session at a time, buffers released lazily | 13.9 s | 6.8 GB |
| VAE in fp32 on the CPU | 23.3 s | 6.8 GB |
| 512, all resident | 3.4 s | 6.9 GB |

The VAE, not the UNet, sets the memory at 1024: the fp16 encoder
peaks near 4 GB and the decoder near 6. Dawn keeps a dropped session's
buffers under its default cache; lazy release gives them back for
0.02 s a step.

## What follows

A 16 GB card keeps everything resident at about 11 s a variation. An
8 GB card opens one session at a time on a thin margin, or takes 512
for small holes; a tiled VAE is the piece that makes it comfortable.
Then the plan as §380 has it: the fetch with resume (5.3 GB), the
three sessions behind the retouch's fill with LaMa for the CPU, three
variations by seed, and the harness against Lightroom's Generative
Remove before it ships, with RORem-4S and OSOR-SDXL through the same
pipeline. The timing and removal examples live on the `rorem` branch
under greycard-ai's examples; the exports and scripts are kept out of
the repository.
