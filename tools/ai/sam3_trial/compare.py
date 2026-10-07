"""Compare two runs' masks: fp16 image encoder against fp32, or a
community export against our own.

    python compare.py RUN_A RUN_B

For every frame and phrase present in both, prints the IoU of the
masks thresholded at 0.5 and the largest per-pixel difference, and
flags a phrase found in one run and not the other.
"""

import pathlib
import sys

import numpy as np
from PIL import Image

a, b = map(pathlib.Path, sys.argv[1:3])
ious, flips = [], []
for da in sorted(p for p in a.iterdir() if p.is_dir()):
    db = b / da.name
    for ma in sorted(da.glob("*.png")):
        mb = db / ma.name
        if not mb.exists():
            continue
        x = np.asarray(Image.open(ma), np.float32) / 255
        y = np.asarray(Image.open(mb), np.float32) / 255
        bx, by = x > 0.5, y > 0.5
        if bx.any() != by.any():
            flips.append(f"{da.name}/{ma.stem}: {'A' if bx.any() else 'B'} only")
            continue
        if not bx.any():
            continue
        iou = (bx & by).sum() / (bx | by).sum()
        ious.append(iou)
        if iou < 0.95:
            print(f"{da.name}/{ma.stem}: IoU {iou:.3f}, max diff {np.abs(x - y).max():.3f}")
print(f"{len(ious)} masks in both: IoU median {np.median(ious):.4f}, min {min(ious):.4f}")
print(f"{len(flips)} found in one run only:", *flips, sep="\n  ")
