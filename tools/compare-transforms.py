#!/usr/bin/env python3
"""Render a raw through greycard's display curve both ways and through
ACES 2.0 and AgX, at matched exposure, for looking at side by side.

    python3 -m venv target/venv-ocio
    target/venv-ocio/bin/pip install opencolorio numpy tifffile imagecodecs imageio
    git clone --depth 1 https://github.com/sobotka/AgX target/agx
    (Blender's config and luts copied to target/blender-agx, see tools/agx-oracle.py)
    cargo build --release -p greycard-cli -p greycard-ui
    target/venv-ocio/bin/python tools/compare-transforms.py OUT_DIR RAW...

Writes, per raw, NAME-off.jpg (greycard, per channel), NAME-on.jpg
(greycard, Hold hue to white), NAME-agx-port.jpg (greycard, AgX),
NAME-aces2.jpg (ACES 2.0 SDR Rec.709 from OpenColorIO's built-in
studio config), NAME-agx.jpg and NAME-agx-punchy.jpg (Sobotka's AgX
config, sRGB-only), NAME-agx-blender.jpg and NAME-agx-blender-punchy.jpg
(Blender's wide-gamut AgX, the view "AgX" on the sRGB display without
and with the look "AgX - Punchy", which is what greycard's AgX ports),
all at a 1600 long edge and sRGB. The ACES and AgX renders
start from greycard's own scene-linear Rec.2020 develop (`greycard
develop --output`) and are given the exposure that puts their median
luminance at greycard's per-channel export's, so only the transform
differs. NAME-agx-blender-punchy-same.jpg is Blender's AgX with Punchy
as Blender ships it (white relative exposure 6.5), at the export's own
exposure (the finish's baseline of 0.8 stops and the AgX mode's own
gain, AGX_GAIN below, that puts mid grey where per channel has it);
greycard's port sets its white at the sensor clip instead, so the two
differ there by design, and the comparison that follows measures the
port against Blender's view as shipped: the script prints, per
frame, the median and 99th percentile of the difference between the
two in 8-bit levels, once over a central 60 percent crop of each
resampled to the same 320-pixel grid, and once between their
per-channel quantiles. Blender's sRGB display encodes with sRGB's
piecewise curve, as the export does, so the two files compare as
written. Sobotka's config's sRGB display is a 2.2 power, and its two
columns are written as OCIO gives them, which an sRGB viewer shows
with the deep shadows lifted by up to eight levels; for the record a
second line per frame compares Sobotka's Punchy with Blender's, both
matched to the per-channel export's median and Sobotka's re-encoded
for sRGB, by the same quantile method. The two are not one pipeline (the export applies
the lens profile and the camera's crop and resizes with its output
sharpen; the linear develop does none of that), so the crop figure is
an upper bound that includes the misalignment and the quantile figure
is blind to it. Nothing is written beside the raw: the greycard runs
pass --no-sidecars and keep their settings under OUT_DIR. A raw already
rendered is skipped, and a failed or stuck render (ten minutes) is
printed and skipped, as is a greycard export already there. The
binaries are the release build;
GREYCARD_PROFILE=debug picks the debug one.
"""
import os, subprocess, sys
import numpy as np, tifffile, imageio.v3 as iio, PyOpenColorIO as o

out, raws = sys.argv[1], sys.argv[2:]
profile = os.environ.get("GREYCARD_PROFILE", "release")
root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.makedirs(out, exist_ok=True)
xdg = {k: f"{out}/xdg/{k[4:].lower()}" for k in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME")}
for d in xdg.values():
    os.makedirs(d, exist_ok=True)
env = {**os.environ, **xdg}
M2020to709 = np.array([[1.6605, -0.5876, -0.0728], [-0.1246, 1.1329, -0.0083], [-0.0182, -0.1006, 1.1187]], np.float32)
aces = o.Config.CreateFromBuiltinConfig("studio-config-latest")
agx = o.Config.CreateFromFile(f"{root}/target/agx/config.ocio")
blender = o.Config.CreateFromFile(f"{root}/target/blender-agx/config.ocio")

def srgb_to_lin(c): return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
def lin_to_srgb(c): return np.where(c <= 0.0031308, 12.92 * c, 1.055 * np.maximum(c, 0) ** (1 / 2.4) - 0.055)
def luma(rgb): return rgb[..., 0] * 0.2126 + rgb[..., 1] * 0.7152 + rgb[..., 2] * 0.0722
def through(cfg, src, display, view, img, look=None):
    t = o.DisplayViewTransform(src=src, display=display, view=view)
    if look is None:
        proc = cfg.getProcessor(t)
    else:
        pipe = o.LegacyViewingPipeline()
        pipe.setDisplayViewTransform(t)
        pipe.setLooksOverrideEnabled(True)
        pipe.setLooksOverride(look)
        proc = pipe.getProcessor(cfg)
    p = proc.getDefaultCPUProcessor()
    img = np.ascontiguousarray(img, dtype=np.float32).copy()
    p.applyRGB(img)
    return np.clip(img, 0, 1)
# `finish::BASELINE_EXPOSURE`: the stops the export brightens a scene by
# before its curve, with the sliders at rest; and the gain greycard's AgX
# mode applies inside itself so that mid grey lands where the curve per
# channel puts it, in stops, given to OCIO too. The gain and the white
# are found at construction in `Agx::blender_punchy` (crates/greycard-ui/
# src/agx.rs) and cannot be imported here; these are what the test
# `mid_grey_through_agx_is_per_channels` printed at the commit that set
# the white at the sensor clip (white relative exposure 3.8721 stops).
BASELINE = 0.8
AGX_GAIN = 1.3020976
def center(img, keep=0.6):
    h, w = img.shape[:2]
    return img[int(h * (1 - keep) / 2):int(h * (1 + keep) / 2), int(w * (1 - keep) / 2):int(w * (1 + keep) / 2)]
def shrink(img, width, h):
    # Area resampling to `width` columns and `h` rows, by an integral image.
    ys, xs = np.linspace(0, img.shape[0], h + 1).astype(int), np.linspace(0, img.shape[1], width + 1).astype(int)
    ii = np.zeros((img.shape[0] + 1, img.shape[1] + 1, 3), np.float64)
    ii[1:, 1:] = img.astype(np.float64).cumsum(0).cumsum(1)
    s = ii[ys[1:, None], xs[None, 1:]] - ii[ys[:-1, None], xs[None, 1:]] - ii[ys[1:, None], xs[None, :-1]] + ii[ys[:-1, None], xs[None, :-1]]
    return s / ((ys[1:, None] - ys[:-1, None]) * (xs[None, 1:] - xs[None, :-1]))[..., None]
def matched(fn, target):
    lo, hi = 0.05, 40.0
    for _ in range(30):
        g = (lo * hi) ** 0.5
        lo, hi = (g, hi) if np.median(luma(srgb_to_lin(fn(g)))) < target else (lo, g)
    return (lo * hi) ** 0.5

def render(raw):
    name = os.path.splitext(os.path.basename(raw))[0]
    for mode, flag in (("off", []), ("on", ["--hold-hue"]), ("agx-port", ["--agx"])):
        if os.path.exists(f"{out}/{name}-{mode}.jpg"):
            continue
        subprocess.run([f"{root}/target/{profile}/greycard-ui", raw, "--no-sidecars", "--no-display-profile",
                        *flag, "--long-edge", "1600", "--export", f"{out}/{name}-{mode}.jpg"],
                       env=env, check=True, capture_output=True, timeout=600)
    subprocess.run([f"{root}/target/{profile}/greycard", "develop", raw, "--output", f"{out}/{name}-linear.tif"],
                   check=True, capture_output=True, timeout=600)
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
        "agx-blender": lambda g: through(blender, "Linear Rec.2020", "sRGB", "AgX", lin * g),
        "agx-blender-punchy": lambda g: through(blender, "Linear Rec.2020", "sRGB", "AgX", lin * g, "AgX - Punchy"),
    }
    matched_gain = {}
    for tag, fn in runs.items():
        g = matched(fn, target)
        matched_gain[tag] = g
        iio.imwrite(f"{out}/{name}-{tag}.jpg", (fn(g) * 255 + 0.5).astype(np.uint8), quality=92)
        print(f"{name} {tag}: {np.log2(g):+.2f} stops to match")
    # Blender's Punchy at the export's own exposure, against the port.
    same = (runs["agx-blender-punchy"](2.0 ** (BASELINE + AGX_GAIN)) * 255 + 0.5).astype(np.uint8)
    iio.imwrite(f"{out}/{name}-agx-blender-punchy-same.jpg", same, quality=92)
    # Sobotka's Punchy against Blender's, both matched, by quantiles.
    sob = lin_to_srgb(runs["agx-punchy"](matched_gain["agx-punchy"]) ** 2.2) * 255
    ble = runs["agx-blender-punchy"](matched_gain["agx-blender-punchy"]) * 255
    q = np.linspace(1, 99, 99)
    sq = np.abs(np.stack([np.percentile(sob[..., k], q) - np.percentile(ble[..., k], q) for k in range(3)])).max(axis=0)
    print(f"{name} Sobotka's Punchy against Blender's, matched: quantiles median {np.median(sq):.2f}, p99 {np.percentile(sq, 99):.2f} levels", flush=True)
    port = iio.imread(f"{out}/{name}-agx-port.jpg")
    rows = round(320 * port.shape[0] / port.shape[1])
    crop = np.abs(shrink(center(port), 320, rows) - shrink(center(same), 320, rows)).max(axis=2)
    q = np.linspace(1, 99, 99)
    quant = np.abs(np.stack([np.percentile(port[..., k], q) - np.percentile(same[..., k], q) for k in range(3)])).max(axis=0)
    print(f"{name} agx-port against Blender's AgX Punchy at the same exposure: center crop median "
          f"{np.median(crop):.2f}, p99 {np.percentile(crop, 99):.2f} levels; quantiles median "
          f"{np.median(quant):.2f}, p99 {np.percentile(quant, 99):.2f}", flush=True)

for raw in raws:
    name = os.path.splitext(os.path.basename(raw))[0]
    if os.path.exists(f"{out}/{name}-agx-blender-punchy-same.jpg"):
        continue
    try:
        render(raw)
    except subprocess.TimeoutExpired as e:
        print(f"{raw}: timed out after 600 s in {os.path.basename(e.cmd[0])} {' '.join(a for a in e.cmd if a.startswith('--'))}", flush=True)
    except subprocess.CalledProcessError as e:
        print(f"{raw}: {os.path.basename(e.cmd[0])} failed ({e.returncode}): {(e.stderr or b'').decode(errors='replace').strip()[-400:]}", flush=True)
