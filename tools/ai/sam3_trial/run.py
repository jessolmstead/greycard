"""Run every phrase on every frame from the preset table alone.

    python run.py IMAGE_ENCODER DECODER PRESETS.npz OUT_DIR IMG...

No language encoder is loaded: this is the editor's preset path. For
each frame, the image is encoded once and each phrase decoded; the
instances are merged into one soft mask (the per-pixel max of their
sigmoids), upsampled bilinearly to the frame, and written as an 8-bit
PNG. results.json holds each phrase's instance count, scores and
decode time, and each frame's encode time. A contact sheet per frame
shows every phrase over the picture, captioned with its top score or
"none".
"""

import json
import pathlib
import sys

import numpy as np
from PIL import Image, ImageDraw

from sam3onnx import Sam3, load_presets

enc, dec, table_path, out = map(pathlib.Path, sys.argv[1:5])
images = [pathlib.Path(p) for p in sys.argv[5:]]
sam = Sam3(enc, dec)
table = load_presets(table_path)
out.mkdir(parents=True, exist_ok=True)

TILE = 360
results = {"image_encoder": str(enc), "frames": {}}
for img_path in images:
    im = Image.open(img_path).convert("RGB")
    feats, t_enc = sam.encode_image(im)
    frame = {"encode_s": round(t_enc, 3), "phrases": {}}
    tiles = []
    for phrase, prompt in table.items():
        boxes, scores, masks, t_dec = sam.decode(feats, prompt)
        if len(masks):
            soft = masks[:, 0].max(axis=0)
        else:
            soft = np.zeros((288, 288), np.float32)
        m = Image.fromarray((soft * 255).round().astype(np.uint8)).resize(im.size, Image.BILINEAR)
        d = out / img_path.stem
        d.mkdir(exist_ok=True)
        m.save(d / f"{phrase.replace(' ', '_').replace(chr(39), '')}.png")
        frame["phrases"][phrase] = {
            "found": int(len(scores)),
            "scores": [round(float(s), 3) for s in sorted(scores, reverse=True)],
            "decode_s": round(t_dec, 3),
        }
        if sam.raw:
            frame["phrases"][phrase].update(sam.raw)
        # the tile: the frame dimmed, the mask in red over it
        base = im.copy()
        base.thumbnail((TILE, TILE))
        mm = m.resize(base.size, Image.BILINEAR)
        red = Image.new("RGB", base.size, (255, 40, 40))
        tile = Image.composite(red, Image.eval(base, lambda v: v * 6 // 10), mm.point(lambda v: v * 7 // 10))
        cap = f"{phrase}: {max(scores):.2f} x{len(scores)}" if len(scores) else f"{phrase}: none"
        dr = ImageDraw.Draw(tile)
        dr.rectangle((0, 0, tile.width, 16), fill=(0, 0, 0))
        dr.text((4, 2), cap, fill=(255, 255, 255))
        tiles.append(tile)
    cols = 8
    w, h = tiles[0].size
    rows = -(-len(tiles) // cols)
    sheet = Image.new("RGB", (cols * w, rows * h), (30, 30, 30))
    for k, t in enumerate(tiles):
        sheet.paste(t, ((k % cols) * w, (k // cols) * h))
    sheet.save(out / f"sheet-{img_path.stem}.jpg", quality=88)
    results["frames"][img_path.stem] = frame
    print(f"{img_path.stem}: encode {t_enc:.2f} s, "
          f"{sum(p['found'] > 0 for p in frame['phrases'].values())}/{len(table)} phrases found", flush=True)

(out / "results.json").write_text(json.dumps(results, indent=1))
