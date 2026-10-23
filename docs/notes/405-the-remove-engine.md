# §405. The remove engine

*2026-10-23*

§380's step 3, the first half: RORem through ort on the GPU, as an
engine in greycard-ai with no caller yet. The second half puts it
behind the retouch's fill with LaMa kept for the CPU, then the
harness against Lightroom's Generative Remove before it ships.

## What it is

`Remove` loads on WebGPU only and refuses anything else; a CPU keeps
LaMa. A window up to 1024 a side is mirror-padded to 512 or 1024,
the two sizes the VAE graphs were exported at, with a UNet graph for
each over one shared 5.1 GB data file. The arithmetic is diffusers'
SDXL inpainting at guidance 1.0 and the empty prompt, which RORem
was trained on: the image and the masked image through the encoder
once, the hole to the latent grid by any-pixel pooling so a stroke
thinner than eight pixels is still filled, Euler discrete over the
model's schedule at strength 0.99 (24 UNet calls at 25 steps), the
decode, and the result composited under the hole only. The reviewer
ran diffusers from RORem's own scheduler config and the sigmas match
to 1e-7. Noise comes from a seed through the grain's own generator,
so a variation is a seed.

Against the torch fp32 reference with the same noise the hole scores
38.6 dB at 1024 and 34.3 dB at 512; the 512 figure is fp16 drift
(CUDA fp16 gives 38.7 there against 44.8 at 1024), grain with no
structure, and the test holds it at 33.

## Memory

Neither ort nor wgpu exposes the adapter's memory, so the engine
opens one session at a time and drops each after its part, 12.7 s
at 1024 here against 9.7 s with all three resident, which an
environment variable allows. ONNX Runtime's WebGPU buffer cache
belongs to the shared context, made once a process, so the lazy
release the engine asks for only takes effect when it opened the
context first; in the editor another model usually has, and the
one-at-a-time peak is then near 8.6 GB rather than 6.8. The second
half has to measure inside the editor and pick one cache mode for
the process.

## Honest answers

A card out of memory can answer with an unrelated picture, so the
output outside the hole is checked against the input: the mean
drift may not exceed 0.02 plus twice the window's mean local
gradient, measured on the input. Six windows end to end put an
honest answer at half the bound (a busy 512 crop 0.068 against
0.149), a flat sky near 0.02, and a wrong picture past 0.2. A fixed
0.08 would have refused the busiest honest window. The reviewer
showed the bound can be fooled by a flat answer on a busy
low-contrast window (one of 29 previews) and by the picture shifted
a few pixels (sixteen of 29); a correlation of the outside with the
input closes that cheaply and goes in with the second half.

## The fetch

5.48 GB in ten files. The store's download did not resume, so a
dropped connection restarted a 5 GB file; a file over 256 MiB now
keeps its part, asks for a range on the next try, and hashes the
whole at the end. Hugging Face's CDN honors the range through its
redirect. The model is registered but unlisted until its files are
published at jessolmstead/greycard-rorem, so `models --fetch all`
does not pull it yet; the publish folder with the model card and
the RAIL++-M license is staged for the upload.

## License

The weights carry CreativeML Open RAIL++-M from SDXL inpainting
(§399), with two training sets under non-commercial or academic
terms noted in the paper. The notice shipped with the entry says
so. Fetched on first use from the published repository, as §36
reads it.
