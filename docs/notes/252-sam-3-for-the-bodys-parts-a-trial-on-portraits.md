# 252. SAM 3 for the body's parts: a trial on portraits (2026-10-05)

A trial, not yet built into the editor: SAM 3 (Meta, the SAM License)
asked for the body's parts and clothing by name, on fourteen
portraits, against the plan of §170 (MediaPipe's landmarks, the
EasyPortrait face parser and SAM 2 from landmarks). It wins, and the
roadmap's parts item now rests on it. The scripts and a recipe to
reproduce every number are in `tools/ai/sam3_trial/`; the timings
come from `crates/greycard-ai/examples/sam3_bench.rs`.

**Why it came back.** §36 set SAM 3 aside for two reasons: the
official weights are gated behind a manual approval, and the concept
path needs a 1.4 GB text encoder for two masks. Neither holds. The
license allows redistribution with the license beside the file, so we
convert the weights once and host our own copy, as §159 did for
BiRefNet; access to `facebook/sam3` was granted for the conversion.
And the text encoder only ever turns a phrase into a vector: the
decoder takes the phrase as two inputs, a 32-token mask and a
32 × 256 block of features, which for a fixed menu can be computed
once and shipped as a table, 32 KB a phrase. Fed from the table, the
decoder's output is bit for bit what it is with the encoder run live.
The trial tokenizes with Hugging Face's CLIP tokenizer, checked
against SAM 3's own on all 61 phrases.

**Our export.** `export.py` traces Meta's `sam3.pt` (sha256
`9999e234…`) with Meta's code at Kentaro Wada's ONNX branch
(`wkentaro/sam3`, commit 812de8a: the rotary encoding in cos and sin
instead of complex numbers, a few untraceable calls removed), two
hard-coded CUDA devices switched to the CPU, into three graphs: the
image encoder (uint8 3 × 1008 × 1008 in, the backbone's six maps
out), the language encoder, and the decoder. The decoder is ours. The
published exports cut at SAM 3's 0.5 inside the graph and drop the
rest; ours returns the sixteen best queries, ranked, with the
presence score apart, so the cut is ours to choose. Every graph
matches the PyTorch model to 7e-4 or better on a check frame; two runs
of the export give byte-identical files; and its masks match a
community export (`wkentaro/sam3-onnx-models-v0.3.0`) at IoU 1.0000
on all 117 masks of a run.

**fp16.** The image encoder converted with onnxconverter-common
(`fp16.py`; the converter leaves some Casts targeting fp32 while
recording their outputs as fp16, so each Cast's target is set to the
recorded type afterwards) is 909 MB against 1.8 GB, and its masks
match fp32's at a median IoU of 0.9996, the worst 0.982; one phrase
of 117 crossed the cut, at 0.5006 against 0.4968. A community fp16
file had looked worse (small parts at 0.80 to 0.95, facial skin at
0.60 on one frame), but that file comes from another export; the drift
was the export, not half precision. The encoder ships in fp16.

**The whole frame.** Every phrase on seven portraits from the test
set, at 1008 square. Face, facial skin, skin, hair, eyebrows, eyes,
hands, belt and the garments that were there were found and clean,
scores mostly 0.8 to 0.97; sky was right on all seven, found under
the three skies and absent on the four frames without one. Free
phrases that pick a thing out worked ("the woman on the left" 0.89,
"leopard print coat" 0.96). Phrases for things in no frame (dog,
bicycle, umbrella, red car, snow, wedding dress, hat) came back
empty; over all seven frames the best query for any of them scored
0.31, so 0.5 has a margin. The presence score and the query's do
different work: "teeth" on closed mouths had presence up to 0.72 and
no query over 0.20. What fails is vocabulary, not the cut: "dress"
painted sweaters and tops worn with jeans at 0.50 to 0.81 on six
frames with no dress in them, and "wheat" missed the wheat field. A
menu offers the garments the model separates (sweater, jeans, coat,
boots scored 0.86 to 0.96 where present), not "dress". "Eyelashes"
paints the eye's whole surround and "eye whites" the whole opening;
neither is a preset.

**The iris.** At the whole frame the decoder's 288-cell mask puts an
iris on a few cells, and nothing was found on any frame. So a
cascade, each step a crop of the full-resolution frame encoded at
1008: faces on the frame; "eyes" in each face crop, which comes back
as one instance over both, split into its connected parts; and each
eye cropped at 2.5 times its width. On the twelve visible eyes (two
faces wore sunglasses and correctly gave none), the word "iris" was
found four times at 0.50 to 0.57, but **"iris of the eye" found all
twelve at 0.77 to 0.91**, on blue, grey and brown eyes and in dim
light, down to an eye crop of 131 pixels from a full-length frame.
The masks sit on the limbus and are cut by both lids. Through the
guided filter the editor puts every model mask through, radius a
128th of the crop and ε 1e-3, the 288-grid steps smooth out and a
spur onto the sclera goes; at ε 1e-4 the filter carves the catchlight
out of the iris (to about 0.6), so the iris keeps 1e-3.

**Teeth and lips.** At the whole frame lips scored 0.38 to 0.59 and
teeth 0.37 to 0.77, at and under the cut. The iris's cascade with
"mouth" in place of "eyes" (one instance, cropped at 1.8 times its
width) fixes both. On seven smiling portraits (twelve mouths: a
studio headshot, a couple, three in a field, one on a couch and a
group of five, one face half behind a hand), teeth were found in all
twelve at 0.73 to 0.93, as one instance a tooth, merged into a mask
that stops at the gums and the lips; lips in all twelve at 0.67 to
0.89, both lips with the teeth left out. On the seven closed mouths of
the first set, six gave no teeth (the best query 0.15 to 0.40) and
one gave 0.72 for a real sliver of upper teeth between parted lips.
"Tongue" painted lower teeth on two faces; "gums" was never found.

**Time.** Warm, on the RTX 5070 Ti through the editor's route (ort on
WebGPU, Dawn): the image encoder 0.32 s in fp16 and 0.48 s in fp32,
a decode 0.15 s; on the 32-thread CPU 3.6 s and 0.74 s. So a preset on
a new picture is about half a second and each further preset on it
0.15 s, the encoding kept as the Object mask keeps SAM 2's. The iris
and the mouth are four encodes and four decodes each, about 2 s on
the card and 15 s on the CPU. Reading back sixteen masks instead of
200 took a decode from 0.157 to 0.148 s: the time is the decoder's
arithmetic, not the copy.

**Decided.** SAM 3 carries the body's parts and clothing. EasyPortrait
is dropped: its classes are all asked for by name here, and with it go
§170's two open calls (the backbone reading of the license rule, and
hosting a file from a Sberbank company's bucket). MediaPipe's iris
landmarks are kept as the CPU fallback, where the cascade's 15 s is
too long and a few megabytes run in milliseconds. Hosted under the
SAM License: the fp16 image encoder (909 MB), the decoder (130 MB)
and the preset table, without a text encoder. "Describe your own
mask" comes after, fetching the 1.4 GB text encoder on first use
(its fp16 untried) and saying "not found" under the cut.
