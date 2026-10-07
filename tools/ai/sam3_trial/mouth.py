"""Teeth and lips by cascade, as iris.py does the iris: faces on the
full-resolution frame, "mouth" in each face crop, and the MOUTH
phrases in a crop of each mouth, every crop encoded at SAM 3's 1008.

    python mouth.py IMAGE_ENCODER DECODER PRESETS.npz OUT_DIR FULL_PNG...

Lips scored just over the cut at the whole frame, and none of the
first seven frames showed teeth; this is the check on smiling
portraits. Per mouth, a sheet: the crop, then each phrase's mask over
it, captioned with its top score; the masks at the crop's own
resolution, and mouths.json with every box, crop and score (and,
with our own export, the presence and the best scores under the cut).
"""

import json
import pathlib
import sys

import numpy as np
from PIL import Image, ImageDraw

from prompts import MOUTH
from sam3onnx import Sam3, load_presets

Image.MAX_IMAGE_PIXELS = None

enc, dec, table_path, out = map(pathlib.Path, sys.argv[1:5])
frames = [pathlib.Path(p) for p in sys.argv[5:]]
sam = Sam3(enc, dec)
table = load_presets(table_path)
out.mkdir(parents=True, exist_ok=True)


def square(box, scale, size, min_side):
    x0, y0, x1, y1 = box
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    side = min(max(min_side, scale * max(x1 - x0, y1 - y0)), *size)
    left = int(round(min(max(cx - side / 2, 0), size[0] - side)))
    top = int(round(min(max(cy - side / 2, 0), size[1] - side)))
    return left, top, left + int(side), top + int(side)


def find(im, crop, phrase):
    region = im.crop(crop)
    feats, _ = sam.encode_image(region)
    boxes, scores, _, _ = sam.decode(feats, table[phrase])
    w, h = region.size
    return [(crop[0] + b[0] * w, crop[1] + b[1] * h, crop[0] + b[2] * w, crop[1] + b[3] * h) for b in boxes], \
        [float(s) for s in scores]


def overlay(region, soft, caption, side=420):
    base = region.copy().resize((side, side), Image.LANCZOS)
    m = Image.fromarray((soft * 255).round().astype(np.uint8)).resize((side, side), Image.BILINEAR)
    tile = Image.composite(Image.new("RGB", base.size, (255, 40, 40)), base, m.point(lambda v: v * 6 // 10))
    d = ImageDraw.Draw(tile)
    d.rectangle((0, 0, side, 16), fill=(0, 0, 0))
    d.text((4, 2), caption, fill=(255, 255, 255))
    return tile


log = {}
for path in frames:
    im = Image.open(path).convert("RGB")
    entry = log[path.stem] = {"size": im.size, "faces": []}
    faces, fscores = find(im, (0, 0, *im.size), "face")
    for fi, (fbox, fs) in enumerate(zip(faces, fscores)):
        fcrop = square(fbox, 1.3, im.size, 256)
        mouths, mscores = find(im, fcrop, "mouth")
        face = {"box": [round(v) for v in fbox], "score": round(fs, 3), "mouth": None}
        entry["faces"].append(face)
        if not mouths:
            continue
        mbox = mouths[int(np.argmax(mscores))]
        mcrop = square(mbox, 1.8, im.size, 96)
        region = im.crop(mcrop)
        feats, _ = sam.encode_image(region)
        mouth = {"box": [round(v) for v in mbox], "crop": list(mcrop), "score": round(max(mscores), 3),
                 "crop_px": mcrop[2] - mcrop[0], "phrases": {}, "raw": {}}
        tiles = [overlay(region, np.zeros((288, 288), np.float32), f"{path.stem} face {fi}, {mouth['crop_px']} px")]
        for phrase in MOUTH:
            _, scores, masks, _ = sam.decode(feats, table[phrase])
            soft = masks[:, 0].max(axis=0) if len(masks) else np.zeros((288, 288), np.float32)
            Image.fromarray((soft * 255).round().astype(np.uint8)).resize(region.size, Image.BILINEAR).save(
                out / f"{path.stem}-f{fi}-{phrase.replace(' ', '_')}.png")
            mouth["phrases"][phrase] = [round(float(s), 3) for s in sorted(scores, reverse=True)]
            if sam.raw:
                mouth["raw"][phrase] = sam.raw
            cap = f"{phrase}: {max(scores):.2f} x{len(scores)}" if len(scores) else f"{phrase}: none"
            tiles.append(overlay(region, soft, cap))
        sheet = Image.new("RGB", (420 * 5, 420 * 2))
        for k, t in enumerate(tiles):
            sheet.paste(t, (420 * (k % 5), 420 * (k // 5)))
        sheet.save(out / f"mouth-{path.stem}-f{fi}.jpg", quality=90)
        face["mouth"] = mouth
    n = sum(f["mouth"] is not None for f in entry["faces"])
    print(f"{path.stem}: {len(faces)} faces, {n} mouths", flush=True)

(out / "mouths.json").write_text(json.dumps(log, indent=1))
