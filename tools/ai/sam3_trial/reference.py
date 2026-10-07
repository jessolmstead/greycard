"""The reference the editor's SAM 3 test checks its decode against.

    python reference.py MODELS_DIR TABLE.bin PICTURE OUT.json [PHRASE ...]

Encodes PICTURE with the image encoder the editor ships
(MODELS_DIR/sam3_image_encoder_fp16.onnx) and decodes each PHRASE
(default: face, hair, shoes) from the shipped table (table.py's
sam3_presets.bin) with MODELS_DIR/sam3_decoder.onnx, on the CPU, as
sam3onnx.py does: the picture squashed to 1008 square with Pillow's
bilinear filter, uint8 planes. OUT.json holds, per phrase, the
presence score, all sixteen query scores, how many are over the 0.5
cut, the best query's box, and the mean of its mask.

The picture is written beside OUT.json as OUT.png, RGB, and the JSON
names that file: the test reads the same pixels, where two JPEG
decoders would not quite agree. OUT.1008.png is the picture squashed
as the encoder sees it, for the test to check the editor's port of
Pillow's filter by. None of these files belongs in the repo.

    GREYCARD_MODELS=STORE GREYCARD_SAM3_REFERENCE=OUT.json \\
        cargo test -p greycard-ai --test sam3 -- --ignored
"""

import json
import pathlib
import sys

import numpy as np
import onnxruntime as ort
from PIL import Image

from sam3onnx import Sam3
from table import read_table

CUT = 0.5

if __name__ == "__main__":
    ort.set_default_logger_severity(3)
    models, table_path, picture, out = map(pathlib.Path, sys.argv[1:5])
    phrases = sys.argv[5:] or ["face", "hair", "shoes"]
    table = read_table(table_path)
    sam = Sam3(models / "sam3_image_encoder_fp16.onnx", models / "sam3_decoder.onnx")

    im = Image.open(picture).convert("RGB")
    png = out.with_suffix(".png")
    im.save(png)
    # What the encoder is fed, for the test to check its own squash by.
    squashed = out.with_suffix(".1008.png")
    im.resize((1008, 1008), Image.BILINEAR).save(squashed)
    feats, _ = sam.encode_image(im)
    result = {"picture": str(png.resolve()), "squashed": str(squashed.resolve()),
              "size": list(im.size), "cut": CUT, "phrases": {}}
    feed_names = ("vision_pos_enc_2", "backbone_fpn_0", "backbone_fpn_1", "backbone_fpn_2")
    for phrase in phrases:
        feed = {k: feats[k] for k in feed_names}
        feed.update(table[phrase])
        feed["box_coords"] = np.zeros((1, 1, 4), np.float32)
        feed["box_labels"] = np.ones((1, 1), np.int64)
        feed["box_masks"] = np.ones((1, 1), np.bool_)
        o = dict(zip((x.name for x in sam.decoder.get_outputs()), sam.decoder.run(None, feed)))
        scores = o["scores"]
        result["phrases"][phrase] = {
            "presence": float(o["presence"][0]),
            "scores": [float(s) for s in scores],
            "found": int((scores > CUT).sum()),
            "best_box": [float(v) for v in o["boxes"][0]],
            "best_mask_mean": float(o["masks"][0, 0].mean()),
        }
        r = result["phrases"][phrase]
        print(f"{phrase}: presence {r['presence']:.4f}, best {r['scores'][0]:.4f}, {r['found']} over the cut")
    out.write_text(json.dumps(result, indent=1) + "\n")
