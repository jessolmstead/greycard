# §731. The learned denoiser on X-Trans: one plane at the canonical phase (2026-11-22)

Each tier now has an X-Trans network beside its Bayer one. An edit
names the tier; the frame's mosaic picks the network. The X-Trans
network does the Bayer network's job in the Bayer network's slot (§37):
it replaces Markesteijn and the profiled denoise between `prepare` and
`finish`, after hot pixels, the mosaic's CA correction (§331, §343),
and the highlight rebuild.

## The contract

- **Input** `mosaic`, `[1, 1, h, w]` float32: the mosaic at full
  resolution, each sample stabilized by its own color's noise model
  (the Bayer VST, §37), sample (0, 0) at the canonical phase, the
  layout of `CfaPattern::xtrans()` from its top left.
- **Output** `rgb`, `[1, 3, h, w]`, stabilized, inverted per channel
  with `Vst::inverse_clean`, as on Bayer.
- **Packing in the graph.** The network unshuffles the plane by 2 into
  four planes, as Bayer's are, and puts the color masks beside them,
  unshuffled the same way (packing D of the trial). The masks are a 6x6
  tile multiplied onto a ones plane and put through DepthToSpace, so
  the runtime hands over one plane and nothing else. The input's name
  tells the runtime which contract a file keeps: `packed` is Bayer,
  `mosaic` is X-Trans.
- **Sides** are whole multiples of 48: the period of 6 times the
  network's 16 (the unshuffle and three levels down). The runtime cuts
  the tile to the nearest 48 and the margin up to a multiple of 24.
  Default tile 1008 and margin 96, 1200 a side; the small tile 528,
  720 a side; Core ML fixed at 1200.
- **Metadata** `greycard.model = denoise-xtrans`, `greycard.version =
  xtrans-<tier>-1`.

## Why one plane at the canonical phase

The 6x6 period has 18 distinct phases, not 36: the pattern is itself
again under a (3, 3) shift. Every flip and transpose of it is one of
its shifts. The runtime starts every tile where the frame reads as the
canonical pattern, so the network learns one phase, as the Bayer one
learns RGGB. A crop, or a sensor's own layout, is a shift, found by
`xtrans_offset`.

At the edges the runtime mirrors about the sample nearest each edge
whose canonical coordinate is 1 mod 3. The pattern is symmetric about
its columns and rows 1 and 4, so a mirrored sample has the color the
pattern has where it was asked for. A frame narrower than the margin
folds back and forth between the two axes. The Bayer mirror's rule,
about the edge sample, would put the wrong color in two of every three
phases.

The trial (packings A to F, 8k and 24k steps, two seeds) chose D: within
0.3 dB of the best packing from scratch, a Bayer network's reach (70 to
86 samples, inside the margin of 96), and the Bayer trunk unchanged, so
each tier starts from its Bayer twin's weights. Warm-started, D led
every packing from scratch by 0.6 to 1.5 dB at the same steps.

## The tiers

Each tier is its Bayer twin's shape, warm-started from it, on 13,625
patches: real X-Trans frames (native and exact pairs) and re-mosaiced
Bayer frames from the archive. Fast trained 80k steps at batch 64;
balanced and best 160k at batch 32, the same number of patches.

PSNR in dB on 60 held-out exact pairs (scenes and a body never trained
on), sRGB two stops up. Classical is Markesteijn's three passes and the
default hybrid denoise.

| σ at mid grey | about | classical | fast | balanced | best |
|---|---|---|---|---|---|
| 0.005 | X-T4 ISO 800 | 38.72 | 41.02 | 41.42 | 41.57 |
| 0.01 | ISO 1600–3200 | 36.90 | 39.07 | 39.35 | 39.46 |
| 0.02 | past ISO 6400 | 34.02 | 36.56 | 36.77 | 36.89 |
| 0.04 | two stops past | 29.70 | 33.83 | 34.04 | 34.16 |

At σ 0.04 the blotch (coarse chroma) halves, 14.1 to 6.7–7.6, and SSIM
rises from .820 to .918–.921. Each X-Trans tier leads its classical
path by 0.5 to 0.7 dB more than its Bayer twin leads its own. In the
crops all three are free of Markesteijn's faceting, its shadow mottle,
and its colored specks; no tier shows a maze, worms, false color, a
grid, or a seam.

| | fast | balanced | best |
|---|---|---|---|
| Parameters | 1.24 M | 2.26 M | 5.08 M |
| File (fp32) | 5.0 MB | 9.1 MB | 20.3 MB |
| Multiplies per pixel | 64k | 147k | 326k |
| CPU, s per MP (8 threads) | 0.41 | 0.72 | 1.15 |
| Bayer twin, s per MP | 0.27 | 0.55 | 0.96 |

X-Trans costs 1.2 to 1.5 times its twin per megapixel on the CPU: the
full-resolution head and the packing are fixed costs that weigh most on
the fast tier.

## What best buys

Fast to balanced gains 0.21 to 0.40 dB, as on Bayer. Balanced to best
gains 0.11 to 0.15 dB, about half Bayer's step, at 1.6 times the time
and 2.2 times the file; in the crops it keeps a little more fine texture
and is otherwise hard to tell from balanced. We ship it so the tier
picker means the same on both mosaics; it does not earn its cost on
quality alone.

## On the card

On a GeForce RTX 5070 Ti through WebGPU, a 26 MP frame takes 3.3, 4.7,
and 8.0 s on fast, balanced, and best, and a 40 MP frame 4.2, 6.5, and
11.0 s. The card's peak at the 1024 tile is 2.1, 3.4, and 5.1 GiB, so
each X-Trans tier has its own row in the memory guard (`XTRANS_PEAKS`),
measured at every tile size. Best steps down to a smaller tile on a
card where balanced and fast still run whole.

The files are published beside the Bayer tiers', and the registry
names the commit of their upload (§647).

## Left open

- fp16 copies for Core ML, checked as the Bayer ones were (§481, §489).
  Until then Core ML opens the fp32 file.
- A merge still takes the learned denoiser on Bayer frames only.
- A tier's fetch takes both its networks, so a Bayer-only library
  downloads 5 to 20 MB it does not use.
