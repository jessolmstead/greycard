"""Build the X-Trans training pool: real X-Trans patches (native and x2
exact pairs) and the Bayer frames' targets, re-mosaiced in the trainer,
compact float16, cut once.

    .venv/bin/python xtrans_pool.py --raws DIR --bayer DIR [--bayer DIR ...] \\
        --tool ../../target/debug/examples/xtrans_data --scratch DIR --out POOLDIR train
    .venv/bin/python xtrans_pool.py ... val
    .venv/bin/python xtrans_pool.py ... split      # say what each set holds

Frames are known by their index in the sorted list of RAFs under
``--raws`` and their EXIF (body, ISO, time), read with exiv2; nothing
else of them is written. Each RAF is exported by the data tool
(``greycard-bench``'s ``xtrans_data``) to a scratch directory, windows
are cut at the canonical phase, Markesteijn's three passes are run on
them by the same tool, and the trimmed centers go into the pool:

- native: a window's mosaic and Markesteijn of it, the target;
- exact: Markesteijn of a window twice the side, box-averaged by 2 (the
  scored truth), its mosaic re-made in the trainer;
- bayer: a ``greycard-denoise-data`` frame's x2 and x3 targets, the
  mosaic re-made on X-Trans in the trainer.

A scene is a run of one body's frames less than ten minutes apart. The
held-out set is whole scenes and whole bodies (``--hold-out``), so no
burst trains and scores both. Base-ISO frames train and score; frames
at ISO 640 and up are only listed, for the crop sheets.
"""

import argparse
import datetime
import glob
import json
import os
import random
import subprocess
import tempfile
from collections import Counter
from concurrent.futures import ThreadPoolExecutor

import numpy as np

M = 18  # Markesteijn's context, trimmed off every window
VAL_SIDE = 288
KEYS = ["Exif.Image.Model", "Exif.Photo.ISOSpeedRatings", "Exif.Photo.DateTimeOriginal"]


def survey(raws):
    """The RAFs under ``raws``, each its index in sorted order, body, ISO,
    time, and scene; listed by body and time."""
    files = sorted(glob.glob(os.path.join(raws, "**", "*.RAF"), recursive=True))
    rows = []
    for i, f in enumerate(files):
        args = ["exiv2", "-q", "-Pkv"] + sum((["-g", k] for k in KEYS), []) + [f]
        out = subprocess.run(args, capture_output=True, text=True).stdout
        d = {}
        for line in out.splitlines():
            parts = line.split(None, 1)
            if len(parts) == 2:
                d.setdefault(parts[0], parts[1].strip())
        rows.append({"idx": i, "file": f, "model": d.get(KEYS[0], ""), "iso": int(d.get(KEYS[1], "0") or 0),
                     "time": datetime.datetime.strptime(d[KEYS[2]], "%Y:%m:%d %H:%M:%S")})
    rows.sort(key=lambda r: (r["model"], r["time"]))
    scene, prev = 0, None
    for r in rows:
        if prev is None or r["model"] != prev["model"] or (r["time"] - prev["time"]).total_seconds() > 600:
            scene += 1
        r["scene"] = scene
        prev = r
    return rows


def split(rows, args):
    """Training frames (at most ``--per-scene`` a scene), held-out frames,
    and the high-ISO frames."""
    bodies = set(args.bodies) if args.bodies else None
    held_bodies = set(args.hold_out_body)
    held_scenes = {int(s) for s in args.hold_out}
    rng = random.Random(7)
    train, val, high, by_scene = [], [], [], {}
    for r in rows:
        if bodies is not None and r["model"] not in bodies:
            continue
        if r["iso"] <= args.base_iso:
            if r["model"] in held_bodies or r["scene"] in held_scenes:
                val.append(r)
            else:
                by_scene.setdefault(r["scene"], []).append(r)
        elif r["iso"] >= 640:
            high.append(r)
    for rs in by_scene.values():
        rng.shuffle(rs)
        train.extend(rs[: args.per_scene])
    return train, val, high


class Tool:
    """The data tool, each run under ``--wrap`` if one is given."""

    def __init__(self, args):
        self.cmd = ([args.wrap] if args.wrap else []) + [args.tool]
        self.scratch = args.scratch

    def run(self, *a):
        r = subprocess.run(self.cmd + list(a), capture_output=True, text=True)
        if r.returncode != 0:
            raise RuntimeError(f"the data tool failed ({r.returncode}): {r.stderr[-300:]}")

    def export(self, file, stem):
        tmp = tempfile.mkdtemp(dir=self.scratch)
        self.run("export", file, stem, tmp)
        meta = json.load(open(os.path.join(tmp, stem + ".json")))
        mosaic = np.load(os.path.join(tmp, stem + ".mosaic.npy"))
        for name in os.listdir(tmp):
            os.remove(os.path.join(tmp, name))
        os.rmdir(tmp)
        return meta, mosaic

    def develop(self, pieces, noise, gains, tag):
        """Markesteijn's three passes on each mosaic piece: RGB [H, W, 3]."""
        lst = os.path.join(self.scratch, f"{tag}.list")
        a, b = noise["a"], noise["b"]
        paths = [(os.path.join(self.scratch, f"{tag}.{i}.in.npy"), os.path.join(self.scratch, f"{tag}.{i}.out.npy"))
                 for i in range(len(pieces))]
        with open(lst, "w") as f:
            for p, (src, dst) in zip(pieces, paths):
                np.save(src, p.astype(np.float32))
                f.write(f"{src} {dst} {a[0]} {a[1]} {a[2]} {b[0]} {b[1]} {b[2]} {gains[0]} {gains[1]} {gains[2]} 0\n")
        self.run("develop", lst)
        out = []
        for src, dst in paths:
            out.append(np.load(dst))
            os.remove(src)
            os.remove(dst)
        os.remove(lst)
        return out


def windows(mosaic, n, side, rng, tries=60):
    """n window origins (multiples of 6) of ``side``, not dark, not clipped."""
    h, w = mosaic.shape
    out = []
    for _ in range(tries):
        if len(out) == n:
            break
        y = 6 * rng.randrange(0, (h - side) // 6)
        x = 6 * rng.randrange(0, (w - side) // 6)
        c = np.asarray(mosaic[y : y + side : 3, x : x + side : 3], dtype=np.float32)
        if c.mean() < 0.02 or (c > 0.97).mean() > 0.01:
            continue
        out.append((y, x))
    return out


def box2(rgb):
    h, w, _ = rgb.shape
    return rgb.reshape(h // 2, 2, w // 2, 2, 3).mean(axis=(1, 3))


def frame_patches(tool, r, role, args):
    stem = f"{r['idx']:03d}"
    rng = random.Random(1000 + r["idx"])
    meta, mosaic = tool.export(r["file"], stem)
    if role == "train":
        side, n_nat, n_ex = args.patch, args.native, args.exact
    else:
        side, n_nat, n_ex = VAL_SIDE, 2, 2
    nat = windows(mosaic, n_nat, side + 2 * M, rng)
    ex = windows(mosaic, n_ex, 2 * side + 2 * M, rng)
    pieces = [mosaic[y : y + side + 2 * M, x : x + side + 2 * M] for y, x in nat]
    pieces += [mosaic[y : y + 2 * side + 2 * M, x : x + 2 * side + 2 * M] for y, x in ex]
    out = []
    for k, rgb in enumerate(tool.develop(pieces, meta["noise"], meta["gains"], stem)):
        rgb = rgb[M:-M, M:-M]
        if k < len(nat):
            mos = pieces[k][M:-M, M:-M]
            out.append(("native", rgb, mos, meta["noise"]["a"], meta["noise"]["b"]))
        else:
            quarter = lambda v: [x / 4 for x in v]  # noqa: E731
            out.append(("exact", box2(rgb), None, quarter(meta["noise"]["a"]), quarter(meta["noise"]["b"])))
    print(f"{stem} {meta['model']} ISO {meta['iso']} {role}: {len(nat)} native, {len(ex)} exact", flush=True)
    return r["idx"], meta, out


def bayer_patches(dirs, side, n_per, seed=3):
    """Patches of the Bayer frames' x2 and x3 targets, with their noise."""
    rng = random.Random(seed)
    stems = sorted(p[:-5] for d in dirs for p in glob.glob(os.path.join(d, "*.json")))
    out = []
    for gi, s in enumerate(stems):
        meta = json.load(open(s + ".json"))
        for k in (2, 3):
            t = np.load(f"{s}.s{k}.target.npy", mmap_mode="r")
            h, w, _ = t.shape
            got = 0
            for _ in range(40):
                if got == n_per // 2:
                    break
                y, x = rng.randrange(0, h - side), rng.randrange(0, w - side)
                c = np.asarray(t[y : y + side, x : x + side], dtype=np.float32)
                if c.mean() < 0.02 or (c > 0.97).mean() > 0.01:
                    continue
                out.append((gi, c, [v / (k * k) for v in meta["noise"]["a"]], [v / (k * k) for v in meta["noise"]["b"]]))
                got += 1
    return out


def save_pool(out_dir, name, entries, sparse):
    """``entries``: (kind, group, rgb [S, S, 3], mosaic [S, S] or None, a, b).
    What ``xtrans.Pool`` reads; ``sparse`` keeps a mosaic only where one
    was cut."""
    os.makedirs(out_dir, exist_ok=True)
    kinds = {"native": 0, "exact": 1, "bayer": 2}
    n, s = len(entries), entries[0][2].shape[0]
    path = lambda ext: os.path.join(out_dir, f"{name}.{ext}")  # noqa: E731
    rgb = np.lib.format.open_memmap(path("rgb.npy"), "w+", np.float16, (n, 3, s, s))
    with_mos = [i for i, e in enumerate(entries) if e[3] is not None] if sparse else list(range(n))
    mos_index = np.full(n, -1, np.int32)
    mos_index[with_mos] = np.arange(len(with_mos))
    mos = np.lib.format.open_memmap(path("mos.npy"), "w+", np.float16, (len(with_mos), s, s))
    kind, group, ab = np.zeros(n, np.int8), np.zeros(n, np.int32), np.zeros((n, 6), np.float32)
    for i, (k, g, r, m, a, b) in enumerate(entries):
        rgb[i] = np.transpose(r, (2, 0, 1)).astype(np.float16)
        if mos_index[i] >= 0:
            mos[mos_index[i]] = m.astype(np.float16) if m is not None else 0
        kind[i], group[i] = kinds[k], g
        ab[i, :3], ab[i, 3:] = a, b
    rgb.flush()
    mos.flush()
    extra = {"mos_index": mos_index} if sparse else {}
    np.savez(path("meta.npz"), kind=kind, group=group, ab=ab, **extra)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("what", choices=["split", "train", "val"])
    p.add_argument("--raws", required=True, help="the X-Trans RAFs, read only")
    p.add_argument("--bayer", action="append", default=[], help="a greycard-denoise-data export directory")
    p.add_argument("--tool", required=True, help="greycard-bench's xtrans_data example")
    p.add_argument("--wrap", help="a command to run the tool under, such as a nice wrapper")
    p.add_argument("--out", required=True, help="the pool's directory")
    p.add_argument("--scratch", required=True, help="where each frame's export goes for a moment: a disk with room, not a small tmpfs")
    p.add_argument("--name", help="the pool's name (default: what)")
    p.add_argument("--bodies", nargs="*", default=[], help="the bodies to use (default: all)")
    p.add_argument("--base-iso", type=int, default=200)
    p.add_argument("--hold-out", nargs="*", default=[], help="scene numbers held out (see split)")
    p.add_argument("--hold-out-body", nargs="*", default=[], help="bodies held out whole")
    p.add_argument("--per-scene", type=int, default=16)
    p.add_argument("--patch", type=int, default=240)
    p.add_argument("--native", type=int, default=24)
    p.add_argument("--exact", type=int, default=12)
    p.add_argument("--bayer-per-frame", type=int, default=40)
    p.add_argument("--workers", type=int, default=4)
    args = p.parse_args()
    rows = survey(args.raws)
    train, val, high = split(rows, args)
    if args.what == "split":
        for name, rs in (("train", train), ("val", val), ("high", high)):
            print(name, len(rs), sorted(Counter((r["model"], r["iso"]) for r in rs).items()))
        scenes = Counter((r["model"], r["scene"]) for r in rows if r["iso"] <= args.base_iso)
        print("base-ISO scenes:", sorted(scenes.items()))
        return
    tool = Tool(args)
    os.makedirs(args.scratch, exist_ok=True)
    name = args.name or args.what
    rs = train if args.what == "train" else val
    entries, metas = [], {}
    with ThreadPoolExecutor(args.workers) as ex:
        for idx, meta, out in ex.map(lambda r: frame_patches(tool, r, args.what, args), rs):
            metas[idx] = {k: meta[k] for k in ("model", "iso", "noise", "gains", "mean")}
            for kind, rgb, mos, a, b in out:
                entries.append((kind, idx, rgb, mos, a, b))
    if args.what == "train":
        for g, rgb, a, b in bayer_patches(args.bayer, args.patch, args.bayer_per_frame):
            entries.append(("bayer", 10000 + g, rgb, None, a, b))
    save_pool(args.out, name, entries, sparse=args.what == "train")
    json.dump(metas, open(os.path.join(args.out, f"{name}.frames.json"), "w"), indent=1)
    print(name, len(entries), "patches")


if __name__ == "__main__":
    main()
