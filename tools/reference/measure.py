"""Measure greycard's renders of the reference frames against
Lightroom's exports and the camera's own JPEG (notes §143).

    python measure.py base SET WORK FRAME[:VARIANT] ...
    python measure.py sliders SET WORK FRAME BASE VARIANT ...
    python measure.py view SCREENSHOT EXPORT

SET is the comparison folder: the raws, and Lightroom's exports under
``lightroom_export/<stem>_<variant>.jpg``. WORK holds greycard's
renders, ``out_<stem>_<variant>.jpg``, as ``render.sh`` writes them;
a variant has the same name on both sides. Needs numpy and Pillow.

``base`` prints each frame's brightness in the camera's JPEG (the
largest JPEG embedded in the raw), greycard's render and Lightroom's
export: the mean encoded value, percentiles of linear luminance, the
share of the picture at white, and the median against the camera's in
stops. ``sliders`` prints, for each variant, the median change in stops
from that editor's own base within percentile bands of its own base
picture, both editors, and writes a side-by-side sheet into WORK.
Bands of each editor's own picture rather than matched pixels, so two
editors' different lens geometry does not matter. ``view`` compares a
fit-view screenshot (``greycard-ui --screenshot``) against the export
scaled to it: the viewport's shader against the CPU reference.

Everything is read as linear luminance from sRGB, downscaled to 1500
pixels wide; a JPEG near black is a few levels, so the darkest tenth's
numbers are the least trustworthy.
"""

import io
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageOps

Image.MAX_IMAGE_PIXELS = None
BANDS = [0, 10, 25, 40, 55, 70, 85, 95, 99, 100]


def luminance(im, width=1500):
    im = im.convert("RGB")
    im = im.resize((width, round(width * im.size[1] / im.size[0])), Image.LANCZOS)
    a = np.asarray(im, np.float32) / 255
    lin = np.where(a <= 0.04045, a / 12.92, ((a + 0.055) / 1.055) ** 2.4)
    y = lin @ np.array([0.2126, 0.7152, 0.0722], np.float32)
    return a, np.clip(y, 1e-4, 1)


def embedded(raw):
    """The largest JPEG a raw carries: the camera's own rendering."""
    data, best, i = Path(raw).read_bytes(), None, 0
    while (i := data.find(b"\xff\xd8\xff", i)) >= 0:
        try:
            im = Image.open(io.BytesIO(data[i:]))
            im.load()
            if best is None or im.size[0] * im.size[1] > best.size[0] * best.size[1]:
                best = im
        except Exception:
            pass
        i += 3
    return ImageOps.exif_transpose(best)


def raw_of(set_dir, stem):
    return next(p for p in Path(set_dir).glob(stem + ".*") if p.suffix != ".gcd")


def base(set_dir, work, frames):
    print(f"{'frame':10} {'':9} {'mean':>6} {'p10':>7} {'median':>7} {'p90':>6} {'p99':>6} {'white':>7}   median vs camera")
    for arg in frames:
        stem, _, variant = arg.partition(":")
        variant = variant or "base"
        rows = [("camera", embedded(raw_of(set_dir, stem))),
                ("greycard", Image.open(Path(work) / f"out_{stem}_{variant}.jpg"))]
        lr = Path(set_dir) / "lightroom_export" / f"{stem}_{variant}.jpg"
        if lr.exists():
            rows.append(("Lightroom", Image.open(lr)))
        camera = None
        for name, im in rows:
            a, y = luminance(im)
            q = np.percentile(y, [10, 50, 90, 99])
            camera = camera or q[1]
            white = (a.max(-1) >= 0.995).mean() * 100
            print(f"{stem:10} {name:9} {a.mean() * 255:6.1f} {q[0]:7.4f} {q[1]:7.3f} {q[2]:6.3f} {q[3]:6.3f} {white:6.2f}%   {np.log2(q[1] / camera):+.2f}")


def response(base_y, y):
    b = np.log2(base_y)
    d = np.log2(y) - b
    q = np.percentile(b, BANDS)
    return [np.median(d[(b >= lo) & (b <= hi)]) for lo, hi in zip(q[:-1], q[1:])]


def sliders(set_dir, work, stem, base_variant, variants):
    gc = lambda v: Image.open(Path(work) / f"out_{stem}_{v}.jpg")
    lr = lambda v: Image.open(Path(set_dir) / "lightroom_export" / f"{stem}_{v}.jpg")
    print(f"{stem}: median change in stops by percentile band of each editor's own base")
    print(f"  {'band':28}" + "".join(f"{f'{a}-{b}':>8}" for a, b in zip(BANDS[:-1], BANDS[1:])))
    bases = {"greycard": luminance(gc(base_variant))[1], "Lightroom": luminance(lr(base_variant))[1]}
    for v in variants:
        for name, im in [("greycard", gc(v)), ("Lightroom", lr(v))]:
            a, y = luminance(im)
            white = (a.max(-1) >= 0.995).mean() * 100
            print(f"  {name + ' ' + v:28}" + "".join(f"{x:+8.2f}" for x in response(bases[name], y)) + f"   white {white:5.2f}%")
    height, rows = 360, []
    for v in [base_variant] + variants:
        pair = []
        for name, im in [("greycard", gc(v)), ("Lightroom", lr(v))]:
            t = im.convert("RGB")
            t.thumbnail((height * 2, height))
            draw = ImageDraw.Draw(t)
            draw.rectangle([0, 0, 260, 20], fill=(0, 0, 0))
            draw.text((4, 4), f"{name} {v}", fill=(255, 255, 255))
            pair.append(t)
        rows.append(pair)
    w = max(p[0].size[0] for p in rows)
    h = max(p[0].size[1] for p in rows)
    sheet = Image.new("RGB", (w * 2 + 10, (h + 10) * len(rows)), (128, 128, 128))
    for r, (a, b) in enumerate(rows):
        sheet.paste(a, (0, r * (h + 10)))
        sheet.paste(b, (w + 10, r * (h + 10)))
    out = Path(work) / f"sheet_{stem}.jpg"
    sheet.save(out, quality=85)
    print(f"  sheet: {out}")


def view(screenshot, export):
    shot = np.asarray(Image.open(screenshot).convert("RGB"), np.float32)
    g = shot.mean(-1)
    background = g[5, 5]
    cols = np.where(np.abs(g - background).max(0) > 3)[0]
    rows = np.where(np.abs(g - background).max(1) > 3)[0]
    x0, x1, y0, y1 = cols[0], cols[-1] + 1, rows[0], rows[-1] + 1
    pic = shot[y0:y1, x0:x1]
    e = Image.open(export).convert("RGB").resize((x1 - x0, y1 - y0), Image.LANCZOS)
    d = (pic - np.asarray(e, np.float32))[4:-4, 4:-4]
    print(f"picture {x1 - x0}x{y1 - y0}: mean abs {np.abs(d).mean():.2f} of 255, signed {d.mean():+.2f}, rms {np.sqrt((d ** 2).mean()):.2f}")


if __name__ == "__main__":
    command, *args = sys.argv[1:]
    if command == "base":
        base(args[0], args[1], args[2:])
    elif command == "sliders":
        sliders(args[0], args[1], args[2], args[3], args[4:])
    elif command == "view":
        view(args[0], args[1])
    else:
        sys.exit(__doc__)
