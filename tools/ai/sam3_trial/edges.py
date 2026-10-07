"""The iris masks at the frame's own pixels, before and after the
guided filter the editor puts every model mask through (He, Sun and
Tang; greycard_core::guided, with the picture's luma as the guide).

    python edges.py IRIS_OUT_DIR FULL_DIR OUT.jpg [PHRASE]

For each eye iris.py found, the PHRASE mask (default "iris of the
eye") is read at its crop's resolution and filtered against the
crop's luma, radius a 128th of the crop's side (the editor's Object
mask uses a 256th of a 2048 preview: 8 px), at ε 1e-3 and 1e-4. One
row an eye, a tight window around the iris, each pixel drawn as a
square of SCALE: the photo, the raw mask's 0.5 edge over it, and the
refined masks' 0.5 edges, then the ε 1e-4 matte itself.
"""

import json
import pathlib
import sys

import numpy as np
from PIL import Image, ImageDraw

Image.MAX_IMAGE_PIXELS = None
iris_dir, full_dir, out = map(pathlib.Path, sys.argv[1:4])
phrase = sys.argv[4] if len(sys.argv) > 4 else "iris of the eye"
TILE = 300


def box_mean(x, r):
    """Mean over a (2r+1)² window, edges by the window's own count."""
    h, w = x.shape
    c = np.pad(x, ((1, 0), (1, 0))).cumsum(0).cumsum(1)
    y0 = np.clip(np.arange(h) - r, 0, h)
    y1 = np.clip(np.arange(h) + r + 1, 0, h)
    x0 = np.clip(np.arange(w) - r, 0, w)
    x1 = np.clip(np.arange(w) + r + 1, 0, w)
    s = c[y1][:, x1] - c[y0][:, x1] - c[y1][:, x0] + c[y0][:, x0]
    n = (y1 - y0)[:, None] * (x1 - x0)[None, :]
    return s / n


def guided(i, p, r, eps):
    mi, mp = box_mean(i, r), box_mean(p, r)
    a = (box_mean(i * p, r) - mi * mp) / (box_mean(i * i, r) - mi * mi + eps)
    b = mp - a * mi
    return np.clip(box_mean(a, r) * i + box_mean(b, r), 0, 1)


def edge_overlay(photo, m, color):
    """The photo with the mask's 0.5 boundary drawn in `color`."""
    inside = m > 0.5
    edge = inside & ~(np.roll(inside, 1, 0) & np.roll(inside, -1, 0) & np.roll(inside, 1, 1) & np.roll(inside, -1, 1))
    o = np.asarray(photo).copy()
    o[edge] = color
    return Image.fromarray(o)


log = json.loads((iris_dir / "eyes.json").read_text())
rows = []
for stem, entry in log.items():
    full = None
    for fi, face in enumerate(entry["faces"]):
        for ei, eye in enumerate(face["eyes"]):
            mpath = iris_dir / f"{stem}-f{fi}-e{ei}-{phrase.replace(' ', '_')}.png"
            if not eye["phrases"].get(phrase) or not mpath.exists():
                continue
            if full is None:
                full = Image.open(next(full_dir.glob(f"{stem}.*"))).convert("RGB")
            crop = full.crop(tuple(eye["crop"]))
            m = np.asarray(Image.open(mpath), np.float32) / 255
            luma = np.asarray(crop.convert("L"), np.float32) / 255
            r = max(2, round(crop.width / 128))
            soft3, soft4 = guided(luma, m, r, 1e-3), guided(luma, m, r, 1e-4)
            # A square window around the raw mask, 60% wider than it.
            ys, xs = np.nonzero(m > 0.5)
            cy, cx = (ys.min() + ys.max()) / 2, (xs.min() + xs.max()) / 2
            half = int(0.8 * max(ys.max() - ys.min(), xs.max() - xs.min())) + 4
            win = (max(0, int(cx) - half), max(0, int(cy) - half),
                   min(crop.width, int(cx) + half), min(crop.height, int(cy) + half))
            sl = (slice(win[1], win[3]), slice(win[0], win[2]))
            photo = crop.crop(win)
            panels = [
                photo,
                edge_overlay(photo, m[sl], (255, 40, 40)),
                edge_overlay(photo, soft3[sl], (40, 255, 40)),
                edge_overlay(photo, soft4[sl], (40, 200, 255)),
                Image.fromarray((soft4[sl] * 255).astype(np.uint8)).convert("RGB"),
            ]
            scale = TILE / max(photo.size)
            row = Image.new("RGB", (TILE * len(panels), TILE + 16))
            for k, pnl in enumerate(panels):
                pnl = pnl.resize((round(pnl.width * scale), round(pnl.height * scale)), Image.NEAREST)
                row.paste(pnl, (k * TILE, 16))
            d = ImageDraw.Draw(row)
            d.text((4, 2), f"{stem} eye {ei}: window {photo.width} px, x{scale:.1f}; radius {r}", fill=(255, 255, 255))
            for k, cap in enumerate(["raw (red)", "eps 1e-3 (green)", "eps 1e-4 (blue)", "eps 1e-4 matte"], 1):
                d.text((k * TILE + 4, 2), cap, fill=(255, 255, 255))
            rows.append(row)

sheet = Image.new("RGB", (rows[0].width, sum(r.height for r in rows)))
y = 0
for row in rows:
    sheet.paste(row, (0, y))
    y += row.height
sheet.save(out, quality=92)
print(f"{len(rows)} eyes -> {out}")
