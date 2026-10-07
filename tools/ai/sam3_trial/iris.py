"""The iris by cascade: face, then eyes in the face, then the iris in
each eye, every step a crop of the full-resolution frame encoded at
SAM 3's 1008.

    python iris.py IMAGE_ENCODER DECODER PRESETS.npz OUT_DIR FULL_PNG...

At the whole frame the decoder's 288-cell mask puts an iris on a few
cells even in a head-and-shoulders portrait; a crop of a few times the
eye's width gives it a hundred or more. Faces are found on the whole
frame, eyes on each face crop (so a full-length frame's small face is
enlarged before the eyes are looked for), and the EYE phrases on each
eye crop. Per eye, a sheet: the crop, then each phrase's mask over it,
captioned with its top score; the masks are saved at the crop's own
resolution, and eyes.json holds every box and score.
"""

import json
import pathlib
import sys

import numpy as np
from PIL import Image, ImageDraw
from scipy import ndimage

from prompts import EYE
from sam3onnx import Sam3, load_presets

Image.MAX_IMAGE_PIXELS = None

enc, dec, table_path, out = map(pathlib.Path, sys.argv[1:5])
frames = [pathlib.Path(p) for p in sys.argv[5:]]
sam = Sam3(enc, dec)
table = load_presets(table_path)
out.mkdir(parents=True, exist_ok=True)


def square(box, scale, size, min_side):
    """A square crop box around `box`, `scale` times its longer side,
    held inside the frame."""
    x0, y0, x1, y1 = box
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    side = max(min_side, scale * max(x1 - x0, y1 - y0))
    side = min(side, *size)
    left = int(round(min(max(cx - side / 2, 0), size[0] - side)))
    top = int(round(min(max(cy - side / 2, 0), size[1] - side)))
    return left, top, left + int(side), top + int(side)


def find(im, crop, phrase):
    """Boxes and scores of `phrase` in `crop` of `im`, in `im`'s pixels,
    with the soft masks at the crop's resolution."""
    region = im.crop(crop)
    feats, _ = sam.encode_image(region)
    boxes, scores, masks, _ = sam.decode(feats, table[phrase])
    w, h = region.size
    full = [(crop[0] + b[0] * w, crop[1] + b[1] * h, crop[0] + b[2] * w, crop[1] + b[3] * h) for b in boxes]
    return full, [float(s) for s in scores], masks, feats


def overlay(region, soft, caption, side=420):
    base = region.copy().resize((side, side), Image.LANCZOS)
    m = Image.fromarray((soft * 255).round().astype(np.uint8)).resize((side, side), Image.BILINEAR)
    red = Image.new("RGB", base.size, (255, 40, 40))
    tile = Image.composite(red, base, m.point(lambda v: v * 6 // 10))
    d = ImageDraw.Draw(tile)
    d.rectangle((0, 0, side, 16), fill=(0, 0, 0))
    d.text((4, 2), caption, fill=(255, 255, 255))
    return tile


log = {}
for path in frames:
    im = Image.open(path).convert("RGB")
    entry = log[path.stem] = {"size": im.size, "faces": []}
    faces, fscores, _, _ = find(im, (0, 0, *im.size), "face")
    for fi, (fbox, fs) in enumerate(zip(faces, fscores)):
        fcrop = square(fbox, 1.3, im.size, 256)
        eyes, escores, emasks, _ = find(im, fcrop, "eyes")
        face = {"box": [round(v) for v in fbox], "score": round(fs, 3), "eyes": []}
        entry["faces"].append(face)
        # "eyes" comes back as one instance over both, so each eye is a
        # connected part of the merged mask, boxed in the frame's pixels.
        split = []
        if len(emasks):
            labels, n = ndimage.label(emasks[:, 0].max(axis=0) > 0.5)
            side = fcrop[2] - fcrop[0]
            for sl in ndimage.find_objects(labels):
                ys, xs = sl
                box = (fcrop[0] + xs.start * side / 288, fcrop[1] + ys.start * side / 288,
                       fcrop[0] + xs.stop * side / 288, fcrop[1] + ys.stop * side / 288)
                if (xs.stop - xs.start) >= 3:
                    split.append(box)
        es = max(escores) if escores else 0.0
        for ei, ebox in enumerate(split):
            ecrop = square(ebox, 2.5, im.size, 96)
            region = im.crop(ecrop)
            feats, _ = sam.encode_image(region)
            eye = {"box": [round(v) for v in ebox], "crop": list(ecrop), "score": round(es, 3),
                   "crop_px": ecrop[2] - ecrop[0], "phrases": {}}
            tiles = [overlay(region, np.zeros((288, 288), np.float32), f"{path.stem} face {fi} eye {ei}, {eye['crop_px']} px")]
            for phrase in EYE:
                _, scores, masks, _ = sam.decode(feats, table[phrase])
                soft = masks[:, 0].max(axis=0) if len(masks) else np.zeros((288, 288), np.float32)
                Image.fromarray((soft * 255).round().astype(np.uint8)).resize(region.size, Image.BILINEAR).save(
                    out / f"{path.stem}-f{fi}-e{ei}-{phrase.replace(' ', '_')}.png")
                eye["phrases"][phrase] = [round(float(s), 3) for s in sorted(scores, reverse=True)]
                if sam.raw:
                    eye.setdefault("raw", {})[phrase] = sam.raw
                cap = f"{phrase}: {max(scores):.2f} x{len(scores)}" if len(scores) else f"{phrase}: none"
                tiles.append(overlay(region, soft, cap))
            sheet = Image.new("RGB", (420 * len(tiles), 420))
            for k, t in enumerate(tiles):
                sheet.paste(t, (420 * k, 0))
            sheet.save(out / f"eye-{path.stem}-f{fi}-e{ei}.jpg", quality=90)
            face["eyes"].append(eye)
    n_eyes = sum(len(f["eyes"]) for f in entry["faces"])
    print(f"{path.stem}: {len(faces)} faces, {n_eyes} eyes", flush=True)

(out / "eyes.json").write_text(json.dumps(log, indent=1))
