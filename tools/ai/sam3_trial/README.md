# SAM 3 trial: body parts, clothing and phrases by name

Asks whether SAM 3 by concept should carry the body's parts and
clothing (in place of MediaPipe, EasyPortrait and SAM 2 from
landmarks), and whether a free-text "Describe" field can be trusted
to say "not found" for a thing the picture does not hold.

What it checks:

1. **Presets without the text encoder.** `presets.py` computes each
   phrase's two decoder inputs once and asserts the decoder gives the
   same output from the saved table as from the live encoder.
2. **Masks and absence.** `run.py` runs every phrase on every frame
   from the table alone, and writes a contact sheet per frame, the
   soft masks, and each phrase's instance scores. With our own export
   it also records the presence score and the five best scores, over
   the cut or not.
3. **The iris.** `iris.py` finds faces on the full-resolution frame,
   eyes in each face crop (one instance over both eyes, split into
   its connected parts), and the eye phrases in a crop of each eye.
4. **Teeth and lips.** `mouth.py` does the same with "mouth" in each
   face crop and the mouth phrases in a crop of each mouth;
   `edges.py` draws the iris masks at the frame's own pixels before
   and after the editor's guided filter.
5. **fp16 against fp32.** The same run with an fp16 image encoder
   (`fp16.py`), then `compare.py` on the two runs.
6. **Time.** Encode and decode seconds on the CPU, in results.json;
   on WebGPU through the editor's ONNX Runtime with
   `crates/greycard-ai/examples/sam3_bench.rs`.

Scripts are run with `python -s` (not `-I`, which would drop this
directory from the import path). `sam3onnx.py` is the runtime side and
is not named `sam3` so that it does not shadow Meta's package.

## Models

Scratch, under `target/sam3-trial/models`, never committed.

- `official/sam3.pt`: Meta's checkpoint from `facebook/sam3` (gated;
  the account must have been granted access), sha256 `9999e234…`.
- `ours/`: our export of it (`export.py`), three graphs, with the
  decoder returning the sixteen best queries, ranked, and the
  presence score apart.
- `wkentaro/`: `wkentaro/sam3-onnx-models-v0.3.0`, and `rusen/`: the
  fp16 image encoder of `rusen/sam3-browser-int8`; community exports
  used before access came, both cut at 0.5 inside the graph.

All of them are SAM 3 and fall under the SAM License, whatever a
card says. The tokenizer is OpenAI CLIP's
(`openai/clip-vit-base-patch32`), checked by `export.py` to give SAM
3's own tokens on every phrase in `prompts.py`.

## The export

In a venv made from `requirements-export.txt`, with Meta's code from
Kentaro Wada's ONNX branch beside it, and two lines that hard-code
CUDA switched to the CPU:

    git clone https://github.com/wkentaro/sam3.git sam3-src
    git -C sam3-src checkout 812de8a683c4f95970dfbff705d91074f48fc2a6
    sed -i '47s/device="cuda"/device="cpu"/' sam3-src/sam3/model/position_encoding.py
    sed -i '281s/device="cuda"/device="cpu"/' sam3-src/sam3/model/decoder.py
    PYTHONPATH=sam3-src python export.py sam3.pt \
        sam3-src/assets/bpe_simple_vocab_16e6.txt.gz img/5F5A0092.jpg \
        models/ours models/clip_tokenizer.json

It prints each graph's largest difference from the PyTorch model on
the check frame, and the tokenizer comparison.

## Frames

Previews from `greycard develop --preview` of the seven portrait
frames the 2026-09-24 model search used (closed mouths), and of seven
smiling portraits from the archive for the teeth (`smile/`): resized
to 2048 on the long side in `img/` for the whole-frame runs, full
resolution in `full/` for the iris and the mouth. The results are in
§252.

    python -s presets.py models/ours presets-ours.npz img/5F5A0092.jpg
    python -s run.py models/ours/sam3_image_encoder.onnx models/ours/sam3_decoder.onnx presets-ours.npz out/ours img/*.jpg
    python -s iris.py models/ours/sam3_image_encoder.onnx models/ours/sam3_decoder.onnx presets-ours.npz out/iris-ours full/*.png
    python -s edges.py out/iris-ours full out/iris-edges.jpg
    python -s mouth.py models/ours/sam3_image_encoder.onnx models/ours/sam3_decoder.onnx presets-ours.npz out/mouth smile/full/*.png
    python -s fp16.py models/ours/sam3_image_encoder.onnx models/ours/sam3_image_encoder_fp16.onnx
    python -s run.py models/ours/sam3_image_encoder_fp16.onnx models/ours/sam3_decoder.onnx presets-ours.npz out/ours16 img/*.jpg
    python -s compare.py out/ours out/ours16
