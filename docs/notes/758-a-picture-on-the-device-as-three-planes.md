# §758. A picture on the device as three planes (2026-11-27)

A picture held on the device in full floats was an Rgba32Float
texture, 16 bytes a pixel, because wgpu has no three-channel float
format. Every shader read `.rgb` and wrote an alpha of 1 that nothing
read. At 102 MP that fourth channel was 400 MB on the upload, and as
much again on each picture Clarity and Dehaze keep (§755).

**Three layers of R32Float.** A picture is now one R32Float texture
of three array layers, 12 bytes a pixel, read as `texture_2d_array`
and written as `texture_storage_2d_array<r32float>`: the upload, the
pictures kept after the local contrast and after the dehaze, and the
sharpened picture. The dehaze, local contrast, sharpen, and lens
shaders load a pixel from the three layers and store to three. The
lens's output band stays Rgba32Float; it is read straight back and
never kept. Nothing asks the device for a feature: r32float storage
and array textures are core WebGPU.

**The accounting.** §755's unified figure at rest goes from 57 to 53
bytes a pixel, and each kept picture from 16 to 12. A discrete card's
figures and the CPU's do not change. On 24 GB at 102 MP with Clarity
and Dehaze the shelf keeps two bases where it kept one.

## Measured

On an RTX 5070 Ti over Vulkan every output is identical to the bit
before and after: the upload's round trip, the sharpen and its mask,
Texture, Clarity, and Structure, the dehaze, and the lens's three
corrections, 30 hashes in all. Nothing is slower past the noise of a
busy machine. At 102 MP a readback went from about 620 ms to 340 to
400, the dehaze from 33 to 23 ms, Clarity from 64 to 52. The lens's
CA path, which samples a channel at a time, went from 12.4 to 4.1 ms
of shader time, since the compiler drops the layers it does not use.

## The CPU base, kept

The open frame's CPU base stands beside its upload, 12 bytes a pixel
each. We did not drop it. A retouch, the learned masks and their
samplers, an export, the shelf at a step to the next frame, and a
turn all read it; the upload is keyed by its identity; and a lost
device is rebuilt from it. `FrameKept::image` holds a second handle
that would have to go too. Each reader would pay a readback, and a
lost device a develop from the raw.

## Left open

- Metal and D3D12 are unrun. A compiler that does not drop unused
  layers would load three times the texels on the lens's CA path; a
  `sample_cubic` that reads one layer would fix it (`lens.wgsl`).
- The 53 is 57 less four, not measured on the Mac.
