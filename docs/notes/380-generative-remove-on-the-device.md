# §380. Generative remove on the device (2026-10-19)

The bar is Lightroom Classic's Generative Remove: a Firefly model in
Adobe's cloud, trained for removal, run at full resolution, three
variations in a few seconds. Our eraser is LaMa in a fixed 512×512
square (§36), and a tester with 45 MP frames calls it barely usable
(issue 30). What it takes to meet the bar, on the device.

## Two gaps, not one

The first is resolution. The Carve export is a fixed square, so a
large patch is filled at a fraction of its pixels and upscaled to a
smear with no grain. LaMa is fully convolutional and holds up well
above its 256 training size, so a dynamic-shape export from the
original checkpoint, a window up to 1024 or 2048, and overlapped
tiles above that fix this with no new model class. It stays the
tool for dust and wires and the path for every machine.

The second is the model. Generic inpainters (SD, SDXL, FLUX Fill)
put a new object in the hole as often as they remove one; Firefly's
edge is training on removal pairs, shadows and reflections included.
The open equivalent is RORem (CVPR 2025, Li et al.): SDXL inpainting
fine-tuned on 200K human-verified removal pairs, 1024 native, about
5 GB in fp16, tagged Apache-2.0 on the SDXL RAIL++-M base. Under
§36's reading of SAM 3's license, a model the user fetches as data
on first use, it qualifies. FLUX.2 klein 4B is Apache but has no
mask input, so not first.

## The runtime

Two sessions through the ort wrapper (UNet, VAE) and a scheduler in
Rust. No text encoder: removal takes no prompt, the model was trained
on a fixed one (the empty string, to confirm from its training code),
so we encode that once and ship the vector as a small tensor beside
the model entry, and the fetch skips the encoder's 1.4 GB. The crop window §36 builds,
at native pixels up to 1024, LaMa's tiling above. A seed per patch,
three seeds per ask for the variations. A grain pass after, from the
pipeline's noise estimate, so the fill carries the frame's noise:
Firefly's output is smooth, so this is where we can be better. The
first check is whether the WebGPU provider loads a 5 GB UNet under
Dawn's buffer limits; CUDA is the safe path.

## Where it runs

SDXL at 1024 and 25 steps is 5 to 10 s a variation on an 8 GB card.
A Mac runs it through Dawn on Metal (§109): a base M5 at 5 TFLOPS is
30 to 60 s a variation, a Max 10 to 20 s; unified memory holds the
weights. The CoreML provider (§90) would cut that by several times
and pays for the denoiser too. CPU-only machines are minutes a
variation, so the tool is not offered there; they keep LaMa.

## The harness first

Twenty to thirty cases: dust, wires, a person at the edge, a person
over a fence, bricks, foliage, sky, a hole touching the frame edge.
Each through Lightroom's Generative Remove and through us, blind
pairs, scored, as the AgX judging ran (§232). Ship at a tie or win on
most and no bad loss. Expect ties on isolated objects over texture
and sky, losses on people cut by the frame, hair and text.

## Order

1. LaMa at the frame's resolution (issue 30).
2. The harness.
3. RORem through ort, GPU providers only, three variations.
4. CoreML, for the Macs.
