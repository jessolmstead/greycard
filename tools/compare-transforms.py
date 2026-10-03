#!/usr/bin/env python3
"""Render a raw through greycard's display curve both ways and through
ACES 2.0 and AgX, at matched exposure, for looking at side by side.

    python3 -m venv target/venv-ocio
    target/venv-ocio/bin/pip install opencolorio numpy tifffile imagecodecs imageio
    git clone --depth 1 https://github.com/sobotka/AgX target/agx
    cargo build -p greycard-cli -p greycard-ui
    target/venv-ocio/bin/python tools/compare-transforms.py OUT_DIR RAW...

Writes, per raw, NAME-off.jpg (greycard, per channel), NAME-on.jpg
(greycard, Hold hue to white), NAME-aces2.jpg (ACES 2.0 SDR Rec.709
from OpenColorIO's built-in studio config), NAME-agx.jpg and
NAME-agx-punchy.jpg (Sobotka's AgX config), all at a 1600 long edge
and sRGB. The ACES and AgX renders start from greycard's own
scene-linear Rec.2020 develop (`greycard develop --output`) and are
given the exposure that puts their median luminance at greycard's
per-channel export's, so only the transform differs. Nothing is
written beside the raw: the greycard runs pass --no-sidecars and keep
their settings under OUT_DIR.
"""
import os, subprocess, sys
import numpy as np, tifffile, imageio.v3 as iio, PyOpenColorIO as o

out, raws = sys.argv[1], sys.argv[2:]
root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.makedirs(out, exist_ok=True)
xdg = {k: f"{out}/xdg/{k[4:].lower()}" for k in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME")}
for d in xdg.values():
    os.makedirs(d, exist_ok=True)
env = {**os.environ, **xdg}
M2020to709 = np.array([[1.6605, -0.5876, -0.0728], [-0.1246, 1.1329, -0.0083], [-0.0182, -0.1006, 1.1187]], np.float32)
aces = o.Config.CreateFromBuiltinConfig("studio-config-latest")
agx = o.Config.CreateFromFile(f"{root}/target/agx/config.ocio")

def srgb_to_lin(c): return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
def luma(rgb): return rgb[..., 0] * 0.2126 + rgb[..., 1] * 0.7152 + rgb[..., 2] * 0.0722
def through(cfg, src, display, view, img):
    p = cfg.getProcessor(o.DisplayViewTransform(src=src, display=display, view=view)).getDefaultCPUProcessor()
    img = np.ascontiguousarray(img, dtype=np.float32).copy()
    p.applyRGB(img)
    return np.clip(img, 0, 1)
def matched(fn, target):
    lo, hi = 0.05, 40.0
    for _ in range(30):
        g = (lo * hi) ** 0.5
        lo, hi = (g, hi) if np.median(luma(srgb_to_lin(fn(g)))) < target else (lo, g)
    return (lo * hi) ** 0.5

for raw in raws:
    name = os.path.splitext(os.path.basename(raw))[0]
    for mode, flag in (("off", []), ("on", ["--hold-hue"])):
        subprocess.run([f"{root}/target/debug/greycard-ui", raw, "--no-sidecars", "--no-display-profile",
                        *flag, "--long-edge", "1600", "--export", f"{out}/{name}-{mode}.jpg"],
                       env=env, check=True, capture_output=True)
    subprocess.run([f"{root}/target/debug/greycard", "develop", raw, "--output", f"{out}/{name}-linear.tif"],
                   check=True, capture_output=True)
    lin = tifffile.imread(f"{out}/{name}-linear.tif").astype(np.float32) / 65535.0
    os.remove(f"{out}/{name}-linear.tif")
    f = max(1, round(max(lin.shape[:2]) / 1600))
    h, w = (lin.shape[0] // f) * f, (lin.shape[1] // f) * f
    lin = lin[:h, :w].reshape(h // f, f, w // f, f, 3).mean(axis=(1, 3))
    ref = iio.imread(f"{out}/{name}-off.jpg").astype(np.float32) / 255.0
    target = np.median(luma(srgb_to_lin(ref)))
    runs = {
        "aces2": lambda g: through(aces, "Linear Rec.2020", "sRGB - Display", "ACES 2.0 - SDR 100 nits (Rec.709)", lin * g),
        "agx": lambda g: through(agx, "Linear BT.709", "sRGB", "AgX", (lin * g) @ M2020to709.T),
        "agx-punchy": lambda g: through(agx, "Linear BT.709", "sRGB", "Appearance Punchy", (lin * g) @ M2020to709.T),
    }
    for tag, fn in runs.items():
        g = matched(fn, target)
        iio.imwrite(f"{out}/{name}-{tag}.jpg", (fn(g) * 255 + 0.5).astype(np.uint8), quality=92)
        print(f"{name} {tag}: {np.log2(g):+.2f} stops to match")
