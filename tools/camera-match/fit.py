"""Fit the camera's picture style as a look, from the JPEG each raw
carries, against greycard's own develop (docs/camera-match.md).

    python fit.py render FRAMES WORK            # develop each raw to a 16-bit TIFF
    python fit.py pairs WORK                    # register and sample block pairs
    python fit.py exposure WORK                 # per-frame offset in stops, re-render
    python fit.py fit WORK NAME                 # matrix, curves, LUT; held-out; NAME.cube
    python fit.py check WORK NAME               # a side-by-side and a ΔE map per frame
    python fit.py diagnose WORK NAME            # where the residual lives
    python fit.py cross WORK CUBE               # another set's look on this set

FRAMES is a text file of raw paths, one a line. WORK is a folder on
disk (not tmpfs) that gets a copy of each raw, its sidecar, the
render, the block pairs and the results. Needs a release build of
greycard-ui at target/release and the venv at target/camera-match/venv
(numpy, Pillow, scipy, tifffile, imagecodecs).

Every picture is handled in display-referred sRGB at the export's
size. Both sides are averaged over blocks after registration; blocks
with texture, or clipped on either side, are dropped, so what is fitted
is color and tone and not the maker's sharpening or noise reduction.
"""

import io
import json
import os
import subprocess
import sys
from pathlib import Path

import numpy as np
import tifffile
from PIL import Image
from scipy import ndimage
from scipy.interpolate import RBFInterpolator

Image.MAX_IMAGE_PIXELS = None
HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
EXE = ROOT / "target/release/greycard-ui"
CONFIG = ROOT / "target/camera-match/config"
LONG_EDGE = 2048
BLOCK = 24
LUT_SIZE = 33

# ---------------------------------------------------------------- color


def srgb_decode(v):
    v = np.asarray(v, np.float32)
    return np.where(v <= 0.04045, v / 12.92, ((v + 0.055) / 1.055) ** 2.4)


def srgb_encode(v):
    v = np.clip(np.asarray(v, np.float32), 0, None)
    return np.where(v <= 0.0031308, v * 12.92, 1.055 * v ** (1 / 2.4) - 0.055)


def luminance(lin):
    return lin @ np.array([0.2126, 0.7152, 0.0722], np.float32)


M1 = np.array(
    [[0.4122214708, 0.5363325363, 0.0514459929],
     [0.2119034982, 0.6806995451, 0.1073969566],
     [0.0883024619, 0.2817188376, 0.6299787005]], np.float32)
M2 = np.array(
    [[0.2104542553, 0.7936177850, -0.0040720468],
     [1.9779984951, -2.4285922050, 0.4505937099],
     [0.0259040371, 0.7827717662, -0.8086757660]], np.float32)


def oklab(lin):
    """Linear sRGB to Oklab. A ΔE of 0.02 or so is a just-visible step."""
    lms = np.clip(lin, 0, None) @ M1.T
    return np.cbrt(lms) @ M2.T


def delta_e(a_lin, b_lin):
    return np.linalg.norm(oklab(a_lin) - oklab(b_lin), axis=-1)


# --------------------------------------------------------------- render

SIDECAR = {
    "current": {
        "version": 4,
        "light": {"enabled": True, "exposure": 0,
                  "tone": {"enabled": True, "contrast": 1.0, "highlights": 0,
                           "shadows": 0, "whites": 0, "blacks": 0}},
        "lens": {"enabled": True, "profile": True, "distortion": True,
                 "chromatic_aberration": True, "vignetting": True,
                 "manual": 0, "ca_red": 0, "ca_blue": 0, "auto_scale": True,
                 "scale": 1, "defringe": False},
    },
    "history": [], "snapshots": [],
}


NCC_MIN = 0.9


def stems(work, registered=False):
    """The frames in the work folder. With `registered`, only those
    whose JPEG laid over the render with a correlation of NCC_MIN or
    better; a frame under that is not the same picture and is left
    out of the fit rather than allowed to pull it."""
    all_ = sorted(p.stem for p in Path(work).glob("*.CR3")) or \
        sorted(p.stem for p in Path(work).glob("*.RAF"))
    if not registered:
        return all_
    stats = json.loads((Path(work) / "pairs.json").read_text())
    kept = [s for s in all_ if stats[s]["ncc"] >= NCC_MIN]
    for s in all_:
        if s not in kept:
            print(f"  {s} left out: registration ncc {stats[s]['ncc']:.3f}")
    return kept


def raw_of(work, stem):
    return next(p for p in Path(work).iterdir()
                if p.stem == stem and p.suffix.upper() in (".CR3", ".RAF"))


def render_one(work, stem, exposure=0.0):
    raw = raw_of(work, stem)
    side = json.loads(json.dumps(SIDECAR))
    side["current"]["light"]["exposure"] = float(exposure)
    if (Path(work) / "no-vignetting").exists():
        side["current"]["lens"]["vignetting"] = False
    Path(f"{raw}.gcd").write_text(json.dumps(side))
    out = Path(work) / f"{stem}.tif"
    out.unlink(missing_ok=True)
    env = dict(os.environ, XDG_CONFIG_HOME=str(CONFIG))
    cmd = [str(EXE), "--no-display-profile", "--export", str(out),
           "--long-edge", str(LONG_EDGE), str(raw)]
    for attempt in range(2):
        try:
            subprocess.run(cmd, env=env, check=True, timeout=120,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            return out
        except subprocess.TimeoutExpired:
            # Seen once: the editor came up and never exported. A
            # second run of the same command went through.
            print("export hung, retrying", stem, flush=True)
    raise RuntimeError(f"export of {stem} hung twice")


def cmd_render(frames, work, *flags):
    """`--no-vignetting` renders with the lens vignetting correction
    off, remembered in the work folder for the re-render."""
    work = Path(work)
    work.mkdir(parents=True, exist_ok=True)
    if "--no-vignetting" in flags:
        (work / "no-vignetting").touch()
    (CONFIG / "greycard").mkdir(parents=True, exist_ok=True)
    (CONFIG / "greycard/settings.json").write_text('{"export_sharpen":"Off"}')
    for line in Path(frames).read_text().split():
        src = Path(line)
        dst = work / src.name
        if not dst.exists():
            dst.write_bytes(src.read_bytes())
        render_one(work, dst.stem)
        print("rendered", dst.stem, flush=True)


# ------------------------------------------------------------- extract


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
    return best.convert("RGB")


def orientation(raw):
    out = subprocess.run(["exiv2", "-g", "Exif.Image.Orientation", "-Pv", str(raw)],
                         capture_output=True, text=True).stdout.strip()
    return int(out) if out else 1


def oriented(im, o):
    return {3: im.transpose(Image.ROTATE_180), 6: im.transpose(Image.ROTATE_270),
            8: im.transpose(Image.ROTATE_90)}.get(o, im)


# ------------------------------------------------------------ register


def gray_small(a, width=512):
    y = luminance(srgb_decode(a))
    f = y.shape[1] / width
    return ndimage.zoom(y, 1 / f, order=1), f


def ncc(a, b):
    a = a - a.mean()
    b = b - b.mean()
    return float((a * b).sum() / np.sqrt((a * a).sum() * (b * b).sum() + 1e-12))


def register(render, jpeg):
    """The similarity that lays the JPEG over the render: a scale about
    the center and a shift, searched on luminance at 512 wide, refined
    once at 1024. Returns (scale, dy, dx) in render pixels and the NCC."""
    best = None
    for width in (512, 1024):
        r_small, f = gray_small(render, width)
        j_small, _ = gray_small(jpeg, width * jpeg.shape[1] / render.shape[1])
        scales = np.linspace(0.94, 1.06, 25) if best is None else \
            best[0] + np.linspace(-0.006, 0.006, 13)
        shifts = range(-6, 7) if best is None else range(-3, 4)
        cand = None
        for s in scales:
            warped = warp_to(j_small, r_small.shape, s, 0, 0)
            for dy in shifts:
                for dx in shifts:
                    w = np.roll(warped, (dy, dx), (0, 1))
                    c = ncc(w[8:-8, 8:-8], r_small[8:-8, 8:-8])
                    if cand is None or c > cand[3]:
                        cand = (s, dy * f, dx * f, c)
        best = cand
    return best


def warp_to(src, shape, scale, dy, dx):
    """Resample src (any size) onto a grid of `shape`, so that src's
    center maps to the grid's center, src scaled by `scale` × the
    fit-to-shape factor, then shifted by (dy, dx)."""
    sh, sw = src.shape[:2]
    th, tw = shape[:2]
    fit = min(th / sh, tw / sw) * scale
    ys = (np.arange(th) - th / 2 - dy) / fit + sh / 2
    xs = (np.arange(tw) - tw / 2 - dx) / fit + sw / 2
    coords = np.meshgrid(ys, xs, indexing="ij")
    if src.ndim == 2:
        return ndimage.map_coordinates(src, coords, order=1, mode="nearest")
    return np.stack([ndimage.map_coordinates(src[..., c], coords, order=1, mode="nearest")
                     for c in range(src.shape[2])], -1)


def blocks(a, block=BLOCK):
    h, w = a.shape[0] // block * block, a.shape[1] // block * block
    a = a[:h, :w].reshape(h // block, block, w // block, block, -1)
    return a.mean((1, 3)), a.std((1, 3))


def load_render(path):
    return tifffile.imread(path).astype(np.float32) / 65535


def cmd_pairs(work):
    work = Path(work)
    stats = {}
    for stem in stems(work):
        render = load_render(work / f"{stem}.tif")
        raw = raw_of(work, stem)
        jpeg = np.asarray(oriented(embedded(raw), orientation(raw)), np.float32) / 255
        scale, dy, dx, c = register(render, jpeg)
        warped = warp_to(jpeg, render.shape, scale, dy, dx)
        rm, rs = blocks(render)
        jm, js = blocks(warped)
        ry, jy = luminance(srgb_decode(rm)), luminance(srgb_decode(jm))
        smooth = (luminance(rs) < 0.035) & (luminance(js) < 0.035)
        # Inside the JPEG's own frame after the warp.
        inside = warp_to(np.ones(jpeg.shape[:2], np.float32), render.shape, scale, dy, dx)
        im, _ = blocks(inside[..., None])
        inside = im[..., 0] > 0.999
        unclipped = (rm.max(-1) < 0.97) & (jm.max(-1) < 0.97) & \
            (rm.min(-1) > 0.01) & (jm.min(-1) > 0.01)
        keep = smooth & inside & unclipped
        np.savez(work / f"{stem}.pairs.npz", render=rm[keep], jpeg=jm[keep],
                 keep=keep, render_all=rm, jpeg_all=jm)
        offset = float(np.median(np.log2(jy[keep] / ry[keep])))
        stats[stem] = dict(scale=scale, dy=dy, dx=dx, ncc=c, blocks=int(keep.sum()),
                           of=int(keep.size), offset_stops=offset)
        print(f"{stem}: scale {scale:.4f} shift ({dy:+.1f},{dx:+.1f}) ncc {c:.3f} "
              f"blocks {keep.sum()}/{keep.size} offset {offset:+.2f} EV", flush=True)
    (work / "pairs.json").write_text(json.dumps(stats, indent=1))


# ------------------------------------------------------------ exposure


def cmd_exposure(work):
    """Re-render every frame with the median offset applied through the
    Exposure slider, so the shared look does not have to carry the
    camera's per-frame brightness. Then rebuild the pairs."""
    work = Path(work)
    stats = json.loads((work / "pairs.json").read_text())
    offsets = {}
    for stem, s in stats.items():
        offsets[stem] = s["offset_stops"]
        render_one(work, stem, s["offset_stops"])
        print(f"re-rendered {stem} at {s['offset_stops']:+.2f} EV", flush=True)
    (work / "offsets.json").write_text(json.dumps(offsets, indent=1))
    cmd_pairs(work)


# ------------------------------------------------------------------ fit


def load_pairs(work, stems_):
    xs, ys, ids = [], [], []
    for i, stem in enumerate(stems_):
        d = np.load(Path(work) / f"{stem}.pairs.npz")
        xs.append(d["render"])
        ys.append(d["jpeg"])
        ids.append(np.full(len(d["render"]), i))
    return np.concatenate(xs), np.concatenate(ys), np.concatenate(ids)


class Model:
    """render (encoded sRGB) → matrix in linear → per-channel curve →
    encoded → residual LUT correction (RBF, sampled to a grid)."""

    def __init__(self):
        self.M = np.eye(3, dtype=np.float32)
        self.curve = None      # (3, K) values at CURVE_X, in linear
        self.rbf = None
        self.rbf_pts = None

    CURVE_X = np.concatenate([[0], np.geomspace(1e-3, 1, 24)]).astype(np.float32)

    RIDGE = 0.02

    def fit_matrix(self, x_lin, y_lin):
        """Least squares with a ridge toward identity: a set that does
        not cover a color, twenty frames of one warm room say, leaves
        that column free and plain least squares runs off (a blue
        diagonal of 0.4 was seen). The ridge is scaled to the data's
        own energy so it is the same pull whatever the set's size."""
        xtx = x_lin.T @ x_lin
        lam = self.RIDGE * np.trace(xtx) / 3
        eye = np.eye(3, dtype=np.float64)
        self.M = np.linalg.solve(xtx + lam * eye, x_lin.T @ y_lin + lam * eye).T.astype(np.float32)

    def fit_curves(self, x_lin, y_lin):
        """Per-channel monotone curves on top of the matrix, in the
        encoded domain where the knots are evenly useful: binned
        medians of the target against the matrix's output, made
        non-decreasing, then stored as linear values at CURVE_X."""
        z = srgb_encode(np.clip(x_lin @ self.M.T, 0, 1))
        y = srgb_encode(y_lin)
        knots = np.linspace(0, 1, 33, dtype=np.float32)
        curve = np.zeros((3, len(self.CURVE_X)), np.float32)
        for c in range(3):
            bins = np.clip(np.round(z[:, c] * 32).astype(int), 0, 32)
            vals = np.full(33, np.nan, np.float32)
            for k in range(33):
                sel = bins == k
                if sel.sum() >= 30:
                    vals[k] = np.median(y[sel, c])
            ok = ~np.isnan(vals)
            vals = np.interp(knots, knots[ok], vals[ok])
            vals = np.maximum.accumulate(vals)
            # Store as linear-in, linear-out at CURVE_X.
            curve[c] = srgb_decode(np.interp(srgb_encode(self.CURVE_X), knots, vals))
        # Kept only where it helps: a curve fitted from binned medians
        # can be worse than none where the data is thin.
        before = delta_e(x_lin @ self.M.T, y_lin).mean()
        self.curve = curve
        after = delta_e(srgb_decode(self.base(srgb_encode(x_lin))), y_lin).mean()
        if after > before:
            self.curve = None

    def base(self, x_enc):
        """Matrix and curves, encoded in and encoded out."""
        z = srgb_decode(x_enc) @ self.M.T
        if self.curve is not None:
            z = np.stack([np.interp(np.clip(z[..., c], 0, 1), self.CURVE_X, self.curve[c])
                          for c in range(3)], -1)
        return srgb_encode(z)

    def fit_lut(self, x_enc, y_enc, n=12000, smoothing=0.02, seed=0):
        rng = np.random.default_rng(seed)
        idx = rng.choice(len(x_enc), min(n, len(x_enc)), replace=False)
        pts = x_enc[idx]
        res = y_enc[idx] - self.base(pts)
        self.rbf = RBFInterpolator(pts, res, kernel="thin_plate_spline",
                                   smoothing=smoothing, neighbors=200)
        self.rbf_pts = pts

    def correction(self, x_enc):
        if self.rbf is None:
            return np.zeros_like(x_enc)
        flat = x_enc.reshape(-1, 3)
        r = self.rbf(flat).astype(np.float32)
        # Pull the residual to zero away from the data, in encoded units.
        d = nearest_distance(flat, self.rbf_pts)
        r *= np.exp(-(d / 0.08) ** 2)[:, None]
        return r.reshape(x_enc.shape)

    def __call__(self, x_enc):
        return np.clip(self.base(x_enc) + self.correction(x_enc), 0, 1)


def nearest_distance(q, pts):
    from scipy.spatial import cKDTree
    return cKDTree(pts).query(q, k=1)[0].astype(np.float32)


def report(name, x_enc, y_enc, pred_enc, ids, nstems):
    de = delta_e(srgb_decode(pred_enc), srgb_decode(y_enc))
    per = [float(np.mean(de[ids == i])) for i in range(nstems)]
    print(f"  {name:14s} ΔE mean {de.mean():.4f}  p95 {np.percentile(de, 95):.4f}  "
          f"worst frame {max(per):.4f}")
    return de, per


def fit_full(x_enc, y_enc, stages=("matrix", "curves", "lut")):
    m = Model()
    x_lin, y_lin = srgb_decode(x_enc), srgb_decode(y_enc)
    m.fit_matrix(x_lin, y_lin)
    if "curves" in stages:
        m.fit_curves(x_lin, y_lin)
    if "lut" in stages:
        m.fit_lut(x_enc, y_enc)
    return m


def cmd_fit(work, name):
    work = Path(work)
    st = stems(work, registered=True)
    x, y, ids = load_pairs(work, st)
    print(f"{len(st)} frames, {len(x)} block pairs")

    print("fitted on all frames:")
    m = Model()
    x_lin, y_lin = srgb_decode(x), srgb_decode(y)
    report("identity", x, y, x, ids, len(st))
    m.fit_matrix(x_lin, y_lin)
    report("matrix", x, y, m.base(x), ids, len(st))
    m.fit_curves(x_lin, y_lin)
    report("matrix+curves", x, y, m.base(x), ids, len(st))
    m.fit_lut(x, y)
    de, per = report("+LUT", x, y, m(x), ids, len(st))
    print("  matrix:\n" + np.array2string(m.M, precision=4, suppress_small=True))

    print("held out, one frame at a time:")
    held = {s: {} for s in ("matrix", "matrix+curves", "+LUT")}
    for i, stem in enumerate(st):
        tr, te = ids != i, ids == i
        mm = fit_full(x[tr], y[tr], ("matrix",))
        held["matrix"][stem] = float(delta_e(srgb_decode(mm.base(x[te])), srgb_decode(y[te])).mean())
        mm = fit_full(x[tr], y[tr], ("matrix", "curves"))
        held["matrix+curves"][stem] = float(delta_e(srgb_decode(mm.base(x[te])), srgb_decode(y[te])).mean())
        mm = fit_full(x[tr], y[tr])
        held["+LUT"][stem] = float(delta_e(srgb_decode(mm(x[te])), srgb_decode(y[te])).mean())
        print(f"  {stem}: fitted {per[i]:.4f}  held-out matrix {held['matrix'][stem]:.4f}"
              f"  +curves {held['matrix+curves'][stem]:.4f}  +LUT {held['+LUT'][stem]:.4f}",
              flush=True)
    for k, v in held.items():
        vals = np.array(list(v.values()))
        print(f"  {k:14s} held-out mean {vals.mean():.4f}  worst {vals.max():.4f}")

    write_cube(m, work / f"{name}.cube", name)
    (work / "fit.json").write_text(json.dumps(
        {"matrix": m.M.tolist(), "curve_x": m.CURVE_X.tolist(), "curve": None if m.curve is None else m.curve.tolist(),
         "fitted_per_frame": dict(zip(st, per)), "held_out": held}, indent=1))
    print("wrote", work / f"{name}.cube")


def write_cube(m, path, title, size=LUT_SIZE):
    g = np.linspace(0, 1, size, dtype=np.float32)
    b, gg, r = np.meshgrid(g, g, g, indexing="ij")     # red fastest
    grid = np.stack([r, gg, b], -1).reshape(-1, 3)
    out = m(grid)
    lines = [f"TITLE \"{title}\"", "# encoding: srgb", "# primaries: srgb",
             "# fitted from the camera's embedded JPEG by tools/camera-match/fit.py",
             f"LUT_3D_SIZE {size}", "DOMAIN_MIN 0 0 0", "DOMAIN_MAX 1 1 1"]
    lines += [f"{v[0]:.6f} {v[1]:.6f} {v[2]:.6f}" for v in out]
    Path(path).write_text("\n".join(lines) + "\n")


# ---------------------------------------------------------------- check


def cmd_check(work, name):
    """Apply the LUT to each render in numpy (trilinear) and write a
    sheet: render, render through the look, the camera JPEG, and a ΔE
    map. Also the numbers on the kept blocks."""
    work = Path(work)
    lut = np.loadtxt(work / f"{name}.cube", comments=("#", "TITLE", "LUT", "DOMAIN"),
                     dtype=np.float32)
    n = round(len(lut) ** (1 / 3))
    lut = lut.reshape(n, n, n, 3)                       # [b, g, r]
    stats = json.loads((work / "pairs.json").read_text())
    for stem in stems(work):
        render = load_render(work / f"{stem}.tif")
        s = stats[stem]
        raw = raw_of(work, stem)
        jpeg = np.asarray(oriented(embedded(raw), orientation(raw)), np.float32) / 255
        warped = warp_to(jpeg, render.shape, s["scale"], s["dy"], s["dx"])
        coords = (render[..., ::-1] * (n - 1)).reshape(-1, 3).T
        looked = np.stack([ndimage.map_coordinates(lut[..., c], coords, order=1)
                           for c in range(3)], -1).reshape(render.shape)
        de = delta_e(srgb_decode(looked), srgb_decode(warped))
        de_map = np.clip(de / 0.1, 0, 1)
        de_img = np.stack([de_map, 1 - de_map, np.zeros_like(de_map)], -1)
        row = np.concatenate([render, looked, warped, de_img], 1)
        Image.fromarray((np.clip(row, 0, 1) * 255).astype(np.uint8)).save(
            work / f"check_{stem}.jpg", quality=90)
        print(f"{stem}: full-frame ΔE mean {de.mean():.4f} p95 {np.percentile(de, 95):.4f}",
              flush=True)


def cmd_diagnose(work, name):
    """Where the residual after the look lives: lightness against
    chroma, by radius from the frame's center, and by tone level."""
    work = Path(work)
    lut = np.loadtxt(work / f"{name}.cube", comments=("#", "TITLE", "LUT", "DOMAIN"),
                     dtype=np.float32)
    n = round(len(lut) ** (1 / 3))
    lut = lut.reshape(n, n, n, 3)

    def apply(x):
        c = (x[..., ::-1] * (n - 1)).reshape(-1, 3).T
        return np.stack([ndimage.map_coordinates(lut[..., k], c, order=1)
                         for k in range(3)], -1).reshape(x.shape)

    dL, C, rad, lum = [], [], [], []
    for s in stems(work, registered=True):
        d = np.load(work / f"{s}.pairs.npz")
        keep, ra, ja = d["keep"], d["render_all"], d["jpeg_all"]
        h, w = keep.shape
        yy, xx = np.mgrid[0:h, 0:w]
        r = np.hypot((yy - h / 2) / (h / 2), (xx - w / 2) / (w / 2))
        a, b = oklab(srgb_decode(apply(ra))), oklab(srgb_decode(ja))
        diff = (a - b)[keep]
        dL.append(diff[:, 0])
        C.append(np.hypot(diff[:, 1], diff[:, 2]))
        rad.append(r[keep])
        lum.append(b[keep][:, 0])
    dL, C, rad, lum = map(np.concatenate, (dL, C, rad, lum))
    print(f"after the look: |dL| mean {np.abs(dL).mean():.4f} (signed {dL.mean():+.4f}), "
          f"chroma error mean {C.mean():.4f}")
    for lo, hi in [(0, .3), (.3, .5), (.5, .7), (.7, .9), (.9, 1.5)]:
        m = (rad >= lo) & (rad < hi)
        print(f"  radius {lo:.1f}-{hi:.1f}: dL {dL[m].mean():+.4f}  chroma {C[m].mean():.4f}  n {m.sum()}")
    for lo, hi in [(0, .3), (.3, .5), (.5, .7), (.7, .85), (.85, 1)]:
        m = (lum >= lo) & (lum < hi)
        print(f"  L {lo:.2f}-{hi:.2f}: dL {dL[m].mean():+.4f} |dL| {np.abs(dL[m]).mean():.4f} "
              f"chroma {C[m].mean():.4f}  n {m.sum()}")


def cmd_cross(work, cube):
    """Apply a look fitted elsewhere to this set's kept blocks: how
    well another body's or style's look does here, against this set's
    own. ΔE mean and p95, and the chroma part alone."""
    work = Path(work)
    lut = np.loadtxt(cube, comments=("#", "TITLE", "LUT", "DOMAIN"), dtype=np.float32)
    n = round(len(lut) ** (1 / 3))
    lut = lut.reshape(n, n, n, 3)
    st = stems(work, registered=True)
    x, y, ids = load_pairs(work, st)
    c = (x[:, ::-1] * (n - 1)).T
    pred = np.stack([ndimage.map_coordinates(lut[..., k], c, order=1) for k in range(3)], -1)
    a, b = oklab(srgb_decode(pred)), oklab(srgb_decode(y))
    de = np.linalg.norm(a - b, axis=1)
    chroma = np.hypot(a[:, 1] - b[:, 1], a[:, 2] - b[:, 2])
    per = [float(de[ids == i].mean()) for i in range(len(st))]
    print(f"{Path(cube).stem} on {work.name}: ΔE mean {de.mean():.4f} p95 {np.percentile(de, 95):.4f} "
          f"worst frame {max(per):.4f}; chroma {chroma.mean():.4f}")


if __name__ == "__main__":
    cmd, *args = sys.argv[1:]
    {"render": cmd_render, "pairs": cmd_pairs, "exposure": cmd_exposure,
     "fit": cmd_fit, "check": cmd_check, "diagnose": cmd_diagnose,
     "cross": cmd_cross}[cmd](*args)
