"""Compute the preset table once, and check the decoder takes it.

    python presets.py MODELS_DIR OUT.npz [CHECK_IMAGE]

Runs the language encoder over every phrase in prompts.py and saves
the decoder's two text inputs per phrase. With CHECK_IMAGE, decodes
one phrase both ways (live encoder, loaded table) and asserts the
outputs match, which is the claim the presets rest on.
"""

import pathlib
import sys

import numpy as np
from PIL import Image

from prompts import EYE, FREE, MOUTH, PRESETS
from sam3onnx import Sam3, load_presets, save_presets

models = pathlib.Path(sys.argv[1])
out = pathlib.Path(sys.argv[2])
sam = Sam3(
    models / "sam3_image_encoder.onnx",
    models / "sam3_decoder.onnx",
    models / "sam3_language_encoder.onnx",
    models / "clip_tokenizer.json",
)
table = {p: sam.encode_text(p) for p in dict.fromkeys(PRESETS + FREE + EYE + MOUTH)}
save_presets(out, table)
print(f"{len(table)} phrases, {out.stat().st_size / 1024:.0f} KB at {out}")

if len(sys.argv) > 3:
    feats, _ = sam.encode_image(Image.open(sys.argv[3]))
    loaded = load_presets(out)
    for phrase in ("hair", "sky"):
        a = sam.decode(feats, sam.encode_text(phrase))
        b = sam.decode(feats, loaded[phrase])
        for x, y in zip(a[:3], b[:3]):
            assert x.shape == y.shape and np.array_equal(x, y), phrase
        print(f"{phrase}: live and loaded identical, {len(a[1])} found")
