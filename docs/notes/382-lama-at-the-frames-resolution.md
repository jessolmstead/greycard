# §382. LaMa at the frame's resolution (2026-10-20)

The first step of §380. The fill ran big-lama through Carve's fixed
512 export, so every window was squeezed to 512 and stretched back.

## The export

We export big-lama again from the original checkpoint
(advimman/lama, Apache-2.0) with a dynamic height and width, fp32,
through PyTorch's dynamo exporter (torch 2.14);
`tools/ai/lama_export.py` makes it. Exported as is, the FFC blocks'
`rfftn` and `irfftn` become ONNX `DFT` nodes, which the exporter
writes without complaint and ONNX Runtime 1.30 (the Python package;
we did not try 1.28) fails to run, on a shape mismatch in the
one-sided DFT's output buffer. We write the two transforms as real
matrix products instead, the cosine and sine matrices made from the
tensor's own size inside the graph, as Carve did for its fixed size:
exact to 1e-13 in float64 against `torch.fft`, and every op one the
WebGPU provider runs. The contract is Carve's: image in 0 to 1, mask
1 in the hole, sides multiples of 8 (from 32), the answer composited
over the image in 0 to 1. Opset 21 is the highest that runs whole on
ONNX Runtime 1.28's WebGPU provider; at 22 and 23 some ops fall back
to the CPU, 2.3 times slower at 2048 with eight times the host
memory.

Against Carve's at 512 on a real picture, inside the hole: max
2e-5, mean 1e-6 (one 8-bit level at most). Against PyTorch with
`torch.fft` at 512, 1024, and 2048: max 1.2e-4. It is 0.38 s on the
CPU at 512 where Carve's is 0.86 s.

The file is staged for jessolmstead/greycard-lama with its export
script, LaMa's LICENSE, and a model card; the registry's `FILL` is
now `big-lama-dynamic-fp32` (209,024,270 bytes). A Carve file already
in a store stays in its own directory, 208 MB unused, with no way to
remove it from the editor; the sheet offers the new one. A store
cleanup for retired models is left for later.

| side | WebGPU (RTX 5070 Ti) | card memory | CPU | CPU peak memory |
|---|---|---|---|---|
| 512 | 0.05 s | | 0.38 s | 1.1 GB |
| 1024 | 0.20 s | 2.1 GB | 1.6 s | 3.5 GB |
| 2048 | 0.91 s | 8.6 GB | 6.8 s | 13.9 GB |

So the model is asked at most 1024 a side on every provider: four
tiles of 1024 cost no more than one of 2048, and an 8 GB card or an
8 GB machine runs them.

## What full resolution cannot do

The plan was to fill every window at its own pixels. LaMa does not
allow it: the original model, in PyTorch, fills a sky hole cleanly at
512 and leaves a ghost at 1024 and a dark smear at 2048, the same
hole at more pixels. A sweep over a sky, a shore, and a dust spot
puts the limit on depth, the farthest a hole pixel lies from a known
one: clean to 64 model pixels, a ghost at 96, a smear from 128. So a
window is filled at its own pixels while the hole is no deeper than
64, and otherwise taken down (an area mean, so grain averages rather
than aliases) to the scale that brings it to 64, filled, and taken
back up under the hole alone. A window with no known pixel counts as
deep as it is long. A thin thing, a wire or a post or dust, fills at
full resolution with its grain; a person fills at about what the 512
square gave, which was already near that scale, and comes back as
smooth as it did. The grain pass of §380 is what that wants next.

## The window

The window is the hole's box with half its longer side round it, as
before, but no more than twice the hole's depth, so a long thin
stroke does not drag a frame's worth of context along; rounded to
multiples of 8 within the frame. Past 1024 on a side it goes in
tiles of the same multiple of 8, overlapping by 64 at least and
spread evenly, run in rows. The part of an overlap an earlier tile
holds the most of is handed to the next as known, so the two
continue each other, and the rest ramps between them. Tiles with no
hole are not run. Fills are kept by patch and window, and the window
and depth by patch and picture size: finding them reads the whole
region, 0.09 s for a stroke across a 45 MP frame, which ran three
times on every develop that rebuilt the patched picture and now runs
once a patch. The log says the window, the scale, the tiles, and the
time.

## Out of memory

The WebGPU provider does not report a failed allocation; it answers
with finite garbage, as the denoiser found. What that garbage looks
like in the fill's output is unmeasured: we have not run the fill out
of memory. What keeps an 8 GB card and a Mac clear of it is the cap,
1024 a side, about 2.1 GB of the card's memory. The one check we keep
is sound by construction: outside the hole the export gives the
picture back exactly, so a pixel there off by more than 1e-3 makes
the answer `Implausible`, and the patch is left as it was with a
warning. We tried a bound on the hole's mean against its surround and
dropped it: LaMa rightly fills a dust spot on a bright lamp in a dark
frame far off the surround's mean, and zeroed buffers would pass it
anyway. If a tester meets garbage, a bound measured against a real
failed allocation, as §38 did for the denoiser, is the next step.

## Checked

Unit tests: the window (margin, rounding, the frame's edge, a long
stroke), the depth (square, stroke, diagonal, frame edge, all hole),
the scale, the area shrink, the tiling, ramps that sum to 1, a
carried overlap, an idle tile, and the check outside the hole. Ignored
model tests: the stripes at 512 come back as with Carve (0.845 and
0.244); a 96-wide hole at 1024 matches its surround's color to 0.004
and keeps 67% of its grain; a band across 2600 pixels fills in tiles
with no column step past the field's own. On a 22 MP portrait frame,
a 48-pixel stroke across 3000 pixels of people and water comes back
clean where the 512 square gave blocks, on the GPU and on the CPU;
the couple, the lighthouse, and a post match or beat the old way. A
spot on glinting water gains one glint the old way did not: not a
ghost (the glints removed were elsewhere in the hole), the model
carrying on the water's texture.

Not checked: the editor itself on a frame (the path through
`Ai::fill` is the code the example runs, but no capture), a card
that actually runs out of memory, and the Mac.
