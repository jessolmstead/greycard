#!/usr/bin/env python3
"""Lay out compare-transforms.py's renders as contact sheets for judging.

    target/venv-ocio/bin/python tools/transform-sheets.py OUT_DIR [--wide]

Per frame one row, NAME-sheet.jpg: per channel (today's default), ACES
2.0, AgX Punchy, then AgX base, each labeled; and index.html with
every row in the order the raws were given, so the set is judged by
scrolling. --wide puts each render at 1000 px wide instead of 640. """
import glob, os, sys from PIL import Image, ImageDraw, ImageFont

out = sys.argv[1]
w = 1000 if "--wide" in sys.argv else 640
cols = [("off", "per channel (today)"), ("aces2", "ACES 2.0"), ("agx-punchy", "AgX Punchy"),
        ("agx", "AgX base")]
# The columns as "key=label,key=label": SHEET_BASE replaces the four above,
# SHEET_COLUMNS adds to them, e.g. a second config's renders.
if os.environ.get("SHEET_BASE"):
    cols = []
for kv in [c for c in (os.environ.get("SHEET_BASE", "") + "," + os.environ.get("SHEET_COLUMNS", "")).split(",") if c]:
    k, _, label = kv.partition("="); cols.append((k, label or k))
# Only these frames, one name a line, when given.
only = os.environ.get("SHEET_ONLY")
# A suffix for the sheets and the index, so a second page leaves the first.
suffix = os.environ.get("SHEET_SUFFIX", "")
try:
    font = ImageFont.truetype("/usr/share/fonts/TTF/DejaVuSans.ttf", 22)
except OSError:
    font = ImageFont.load_default()
names = sorted({os.path.basename(p)[:-len("-off.jpg")] for p in glob.glob(f"{out}/*-off.jpg")})
order = []
if os.path.exists(f"{out}/raws.txt"):
    order = [os.path.splitext(os.path.basename(l.strip()))[0] for l in open(f"{out}/raws.txt")]
names = [n for n in order if n in names] + [n for n in names if n not in order]
if only:
    keep = {l.strip() for l in open(only) if l.strip()}
    names = [n for n in names if n in keep]
rows = []
for n in names:
    ims = []
    for key, label in cols:
        p = f"{out}/{n}-{key}.jpg"
        if not os.path.exists(p):
            continue
        im = Image.open(p).convert("RGB")
        im.thumbnail((w, w))
        canvas = Image.new("RGB", (w, im.height + 34), (24, 24, 24))
        canvas.paste(im, ((w - im.width) // 2, 34))
        ImageDraw.Draw(canvas).text((8, 6), label, fill=(230, 230, 230), font=font)
        ims.append(canvas)
    if not ims:
        continue
    h = max(i.height for i in ims)
    sheet = Image.new("RGB", (w * len(ims) + 8 * (len(ims) - 1), h + 34), (24, 24, 24))
    ImageDraw.Draw(sheet).text((8, 6), n, fill=(255, 255, 255), font=font)
    x = 0
    for i in ims:
        sheet.paste(i, (x, 34))
        x += i.width + 8
    sheet.save(f"{out}/{n}-sheet{suffix}.jpg", quality=90)
    rows.append(n)
with open(f"{out}/index{suffix}.html", "w") as f:
    f.write("<html><body style='background:#181818;color:#eee;font-family:sans-serif;margin:0'>\n")
    f.write("<p style='padding:8px'>Columns: " + ", ".join(l for _, l in cols) + ". "
            "Exposure matched by median luminance; framing differs slightly between greycard's export and the linear develop.</p>\n")
    for n in rows:
        f.write(f"<div style='padding:4px 0'><img src='{n}-sheet{suffix}.jpg' style='width:100%'></div>\n")
    f.write("</body></html>\n")
print(f"{len(rows)} sheets, {out}/index{suffix}.html")
