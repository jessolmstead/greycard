#!/usr/bin/env python3
# How straight a thin line or an edge comes out of a render: the position
# of the feature in every row (or column) of a crop, a straight line fitted
# and taken out, then the standard deviation of what is left (the low
# frequency wander, mostly the picture's own) and the root mean square of
# the row-to-row change (the noise's part). Needs ImageMagick.
#
#   scripts/line-wander.py GEOMETRY vline|hline|vedge A B RENDER...
#
# GEOMETRY is ImageMagick's WxH+X+Y. vline/hline: a thin dark line between
# columns/rows A and B, located by the centroid of its darkness. vedge: an
# edge between column A (bright side) and B (dark side), located where the
# row crosses the midpoint. Used in docs/notes.md §13t.
import math
import subprocess
import sys


def load(path, geom):
    out = subprocess.run(
        ["magick", path, "-crop", geom, "+repage", "-colorspace", "Gray", "-depth", "8", "-compress", "none", "pgm:-"],
        capture_output=True, check=True,
    ).stdout.decode().split()
    w, h = int(out[1]), int(out[2])
    v = list(map(int, out[4:]))
    return w, h, [v[y * w:(y + 1) * w] for y in range(h)]


def wander(pos):
    pos = [p for p in pos if p is not None]
    n = len(pos)
    mt = (n - 1) / 2
    mp = sum(pos) / n
    slope = sum((i - mt) * (p - mp) for i, p in enumerate(pos)) / sum((i - mt) ** 2 for i in range(n))
    res = [p - (mp + slope * (i - mt)) for i, p in enumerate(pos)]
    sd = math.sqrt(sum(r * r for r in res) / n)
    step = [res[i + 1] - res[i] for i in range(n - 1)]
    return n, sd, math.sqrt(sum(s * s for s in step) / len(step))


def centroid(seg, start):
    base = max(seg)
    wts = [base - v for v in seg]
    s = sum(wts)
    return None if s == 0 else start + sum(i * w for i, w in enumerate(wts)) / s


def crossing(row, a, b):
    lo = sum(row[max(a - 12, 0):a]) / len(row[max(a - 12, 0):a])
    hi = sum(row[b:b + 12]) / len(row[b:b + 12])
    mid = (lo + hi) / 2
    for i in range(a, b):
        if (row[i] - mid) * (row[i + 1] - mid) <= 0 and row[i] != row[i + 1]:
            return i + (row[i] - mid) / (row[i] - row[i + 1])
    return None


def main():
    geom, kind, a, b = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
    print(f"{'render':24s} {'n':>4s} {'wander sd':>10s} {'step rms':>9s}")
    for path in sys.argv[5:]:
        w, h, img = load(path, geom)
        if kind == "vline":
            pos = [centroid(row[a:b], a) for row in img]
        elif kind == "hline":
            pos = [centroid([img[y][x] for y in range(a, b)], a) for x in range(w)]
        elif kind == "vedge":
            pos = [crossing(row, a, b) for row in img]
        else:
            sys.exit("kind: vline, hline or vedge")
        n, sd, step = wander(pos)
        print(f"{path:24s} {n:4d} {sd:10.3f} {step:9.3f}")


if __name__ == "__main__":
    main()
