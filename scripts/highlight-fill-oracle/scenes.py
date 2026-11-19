# SPDX-License-Identifier: GPL-3.0-or-later
"""Synthetic scenes for the learned highlight fill's fixtures: formulas
only, no photograph and nothing derived from one.

Each scene is a scene-referred picture `truth` (linear Rec.2020 over the
ceiling) clipped per channel at `clips`. The codes count, per CFA cell
(2x2, or 3x3 for the X-Trans-style scene), the colors with any photosite
at or over its clip; `x`, the classical fill's stand-in, is the truth
with each clipped channel held at its clip. Every scene is its own whole
frame, in full-resolution pixels: the shipped model's constants are in
full-size pixels of the 24 to 100 MP frames it was trained on, so a
scene reads as a 96 to 160 px window of such a frame shown whole."""
import numpy as np

CLIPS = (0.93, 1.0, 0.86)


def grid(w, h):
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float64)
    return xx + 0.5, yy + 0.5


def codes_of(truth, clips, cell=2):
    """The count of colors clipped in each pixel's cell."""
    h, w, _ = truth.shape
    hit = truth >= np.asarray(clips)[None, None, :]
    ch, cw = -(-h // cell), -(-w // cell)
    pad = np.zeros((ch * cell, cw * cell, 3), bool)
    pad[:h, :w] = hit
    any_c = pad.reshape(ch, cell, cw, cell, 3).any((1, 3))
    n = any_c.sum(-1).astype(np.uint8)
    return np.repeat(np.repeat(n, cell, 0), cell, 1)[:h, :w]


def clip(truth, clips):
    return np.minimum(truth, np.asarray(clips)[None, None, :]).astype(np.float32)


def make(truth, clips=CLIPS, cell=2):
    return clip(truth, clips), codes_of(truth, clips, cell)


def surround(w, h, lo=0.12, hi=0.5, tint=(1.0, 0.95, 0.85)):
    xx, yy = grid(w, h)
    v = lo + (hi - lo) * (0.5 + 0.5 * np.sin(xx / w * 2.1 + 0.4) * np.cos(yy / h * 1.7))
    return v[..., None] * np.asarray(tint)[None, None, :]


def lamp():
    """A disk lamp: a warm light whose red clips first, then green, so a
    band of one and two colors rings a fully clipped core; a few dark
    specks with negative channels in its ring (demosaic undershoot)."""
    w, h = 96, 80
    xx, yy = grid(w, h)
    r = np.hypot(xx - 50, yy - 40)
    t = surround(w, h, 0.2, 0.55) + (4.0 * np.exp(-(r / 17.0) ** 2))[..., None] * np.array([1.0, 0.62, 0.3])
    x, c = make(t)
    for (py, px) in ((40, 20), (41, 21), (12, 60), (66, 74)):
        x[py, px] = (-0.004, 0.002, -0.001)
    return dict(x=x, codes=c, clips=CLIPS, about='a warm disk lamp, band of one and two colors, negative specks')


def line():
    """A thin specular line, a pixel or two of core across the frame."""
    w, h = 96, 64
    xx, yy = grid(w, h)
    dist = np.abs((yy - 12) - (xx * 0.42)) / np.hypot(1, 0.42)
    t = surround(w, h, 0.3, 0.5) + (2.4 * np.exp(-(dist / 1.3) ** 2))[..., None] * np.array([0.95, 1.0, 1.05])
    x, c = make(t)
    return dict(x=x, codes=c, clips=CLIPS, about='a thin specular line')


def diagonal():
    """Two squares that touch only at a corner at half size (one region,
    8-connected), and two regions whose rings meet: the meeting cells go to
    the larger id, here the left one (it reaches one row lower)."""
    w, h = 96, 64
    t = surround(w, h, 0.45, 0.55, (1.0, 1.0, 1.0))
    t[20:40, 20:40] = (1.5, 1.6, 1.4)
    t[40:60, 40:60] = (1.5, 1.6, 1.4)
    t[10:26, 60:72] = (1.4, 1.2, 1.0)
    t[10:24, 90:96] = (1.0, 1.3, 1.5)
    x, c = make(t)
    return dict(x=x, codes=c, clips=CLIPS, about='diagonal touch at half size; two rings meeting')


def edge_cut():
    """A region cut by the left and bottom frame edges, its core about 60 px
    deep from the top boundary; odd size, so the half-size grid pads by
    the edge and the harmonic's quarter grid drops a row and two columns."""
    w, h = 99, 73
    xx, yy = grid(w, h)
    t = surround(w, h, 0.25, 0.6)
    core = (yy >= 9) & (xx < 80)
    reg = (yy >= 5) & (xx < 84)
    t[reg] = (1.1, 0.97, 0.7)
    t[core] = (1.6, 1.5, 1.3)
    x, c = make(t)
    return dict(x=x, codes=c, clips=CLIPS, about='cut by the frame edge, core about 60 px deep, odd size')


def sky():
    """A broad gradient sky clipped over its upper half, blue clipping first;
    a foreground of bright and dark blocks under it (the harmonic takes
    only the bright)."""
    w, h = 128, 80
    xx, yy = grid(w, h)
    lvl = 2.2 - 1.9 * (yy / 46.0) + 0.15 * np.sin(xx / 23.0)
    t = np.stack([0.80 * lvl, 0.95 * lvl, 1.15 * lvl], -1)
    fg = yy > 46
    blocks = (np.floor(xx / 20) % 2 == 0)
    t[fg & blocks] = (0.5, 0.42, 0.3)
    t[fg & ~blocks] = (0.08, 0.1, 0.14)
    x, c = make(t)
    return dict(x=x, codes=c, clips=CLIPS, about='a broad gradient sky, bright and dark foreground')


def all_clipped():
    """Every cell clipped: no unclipped cell, so every distance is at the
    cap, no ring, and the harmonic has nothing known."""
    w, h = 96, 32
    xx, yy = grid(w, h)
    t = np.stack([1.2 + 0.2 * xx / w, 1.1 + 0.1 * yy / h, 0.9 + 0.3 * xx / w], -1)
    t[:, :30, 2] = 0.8
    x, c = make(t)
    return dict(x=x, codes=c, clips=CLIPS, about='all clipped; distances at the cap')


def no_clip():
    w, h = 96, 48
    x, c = make(surround(w, h, 0.1, 0.8))
    return dict(x=x, codes=c, clips=CLIPS, about='nothing clipped; the fill returns x')


def depth_tie():
    """Three square cores (the distance is chessboard, so a square's depth
    is its half-width) below, inside, and well inside the depth tie (12 to
    48 px): half-widths 12, 20, and 38 px with a band of 3 px, each its own
    color (the edge-cut scene holds the core past the tie)."""
    w, h = 136, 88
    xx, yy = grid(w, h)
    t = surround(w, h, 0.3, 0.5)
    for cx, cy, rad, col in ((24, 17, 12, (1.0, 0.9, 0.8)), (25, 61, 20, (0.85, 1.0, 1.0)),
                             (92, 44, 38, (1.0, 0.8, 0.95))):
        r = np.maximum(np.abs(xx - cx), np.abs(yy - cy))
        lvl = np.where(r < rad, 1.8 + 0.6 * (1 - r / max(rad, 1)), 0.98)
        sel = r < rad + 3
        t[sel] = np.maximum(t[sel], lvl[sel, None] * np.asarray(col)[None, :] * np.array([1.05, 1.0, 1.05]))
    x, c = make(t)
    return dict(x=x, codes=c, clips=CLIPS, about='cores below, at, and inside the depth tie')


def xtrans():
    """A lamp whose codes come in 3x3 cells (X-Trans style), with a few
    untrusted photosites (bit 7), which the fill ignores."""
    w, h = 96, 72
    xx, yy = grid(w, h)
    r = np.hypot((xx - 50) / 1.3, yy - 34)
    t = surround(w, h, 0.25, 0.5) + (3.0 * np.exp(-(r / 14.0) ** 2))[..., None] * np.array([0.7, 1.0, 1.1])
    x, c = make(t, cell=3)
    c[30:33, 40:43] |= 0x80
    c[5, 5] |= 0x80
    return dict(x=x, codes=c, clips=CLIPS, about='3x3-cell codes, untrusted bits')


def mixed():
    """Branches the others miss: a core with no band on a dark surround (no
    bright rim, nothing known to the harmonic nearby), a band with no core,
    2x2 blocks half fully clipped (a band cell by its share, a 3 by its
    largest code), a one-cell region, luminance under the log floor, and a
    band block with negative luminance (demosaic undershoot: its ab at unit
    luminance is clamped, its LMS negative, its luminance at or below 1e-6
    so hl-1 leaves it), and a lone clipped pixel no field reaches."""
    w, h = 96, 64
    xx, yy = grid(w, h)
    t = surround(w, h, 0.3, 0.5)
    t[:, :40] = 0.05
    t[40:44, 2:6] = 1e-5
    x, c = make(t)
    x[6:18, 2:14] = (0.93, 1.0, 0.86)
    c[6:18, 2:14] = 3
    x[30:40, 60:76] = (0.93, 0.8, 0.6)
    c[30:40, 60:76] = 1
    x[32:34, 62:64] = (0.93, -0.4, -0.2)
    for y in range(48, 56, 2):
        for xq in range(50, 66, 2):
            c[y, xq:xq + 2] = 3
            c[y + 1, xq:xq + 2] = 1
            x[y, xq:xq + 2] = (0.93, 1.0, 0.86)
            x[y + 1, xq:xq + 2] = (0.93, 0.9, 0.7)
    c[20:22, 86:88] = 2
    x[20:22, 86:88] = (0.93, 1.0, 0.7)
    # A lone clipped pixel, its block a quarter clipped: no region, no field.
    c[60, 30] = 1
    x[60, 30] = (0.93, 0.7, 0.5)
    return dict(x=x, codes=c, clips=CLIPS, about='core without band on dark, band without core, half-core blocks, '
                                                  'a one-cell region, sub-floor luminance')


HEADROOM = 1.123


def headroom():
    """The lamp on a frame with headroom (a Canon CR3 on master): the input,
    its clips, and x all in units of the saturation, the lamp's divided by
    1.123, so every clip sits below 1. Its fixture stores no x: x is the
    lamp's x / 1.123 (one float32 division a channel), the codes the lamp's."""
    s = lamp()
    clips = tuple(float(np.float32(c / HEADROOM)) for c in CLIPS)
    return dict(x=(s['x'] / np.float32(HEADROOM)).astype(np.float32), codes=s['codes'], clips=clips,
                about='the lamp with headroom 1.123 (x and codes derived from the lamp): clips below 1')


SCENES = dict(lamp=lamp, line=line, diagonal=diagonal, edge_cut=edge_cut, sky=sky, all_clipped=all_clipped,
              no_clip=no_clip, depth_tie=depth_tie, xtrans=xtrans, mixed=mixed, headroom=headroom)
def nonfinite():
    """hl-1's guard (not the trial's): the lamp box-downsampled 3x with a NaN
    channel in the band, an infinite pixel in the core, and a NaN and a
    negative infinity outside. Each stays x bit for bit, and none reaches
    any other pixel."""
    s = lamp()
    x = s['x'][:78].reshape(26, 3, 32, 3, 3).mean((1, 3)).astype(np.float32)
    c = s['codes'][:78, :].reshape(26, 3, 32, 3).max((1, 3)).astype(np.uint8)
    ys, xs = np.nonzero((c == 1) | (c == 2))
    x[ys[0], xs[0], 1] = np.nan
    ys, xs = np.nonzero(c == 3)
    x[ys[len(ys) // 2], xs[len(xs) // 2]] = np.inf
    x[2, 3] = np.nan
    x[24, 30, 2] = -np.inf
    return dict(x=x, codes=c, clips=CLIPS, about="hl-1's guard: NaN and infinite pixels pass through untouched")


def tiny():
    """hl-1's small-frame rule (not the trial's, whose pool raises): 7x6 px,
    a 4x3 half grid with no 4x4 block, so the harmonic is 0 everywhere;
    a clipped spot on a bright surround."""
    w, h = 7, 6
    xx, yy = grid(w, h)
    r = np.hypot(xx - 3.5, yy - 3.0)
    t = surround(w, h, 0.3, 0.6) + (3.0 * np.exp(-(r / 2.2) ** 2))[..., None] * np.array([1.0, 0.8, 0.5])
    x, c = make(t)
    return dict(x=x, codes=c, clips=CLIPS, about="hl-1's small-frame rule: under 8 px a side the harmonic is 0")


def tiles():
    """The port's sparse fields (core keeps them in tiles of 16 half-size
    cells, 32 px): a small lamp straddling the corner where four tiles meet
    (px 32, 32), and a large one whose light stops at a hard edge on px row
    64, a tile row's start, so its clipped pixels on row 63 read a tile
    with no field beside one with a field."""
    w, h = 96, 72
    xx, yy = grid(w, h)
    small = np.hypot(xx - 30, yy - 30)
    large = np.hypot(xx - 70, yy - 52)
    t = (surround(w, h, 0.18, 0.5)
         + (3.0 * np.exp(-(small / 7.0) ** 2))[..., None] * np.array([1.0, 0.8, 0.55])
         + (3.5 * np.exp(-(large / 14.0) ** 2) * (yy < 64))[..., None] * np.array([0.9, 1.0, 0.7]))
    x, c = make(t)
    return dict(x=x, codes=c, clips=CLIPS, about="the port's tiles: a four-tile corner, a field beside an empty tile")


# hl-1's departures from the trial: scenes the trial's code cannot match.
# (`tiles` is no departure, but is written after them so every other
# scene's fixture keeps its bytes.)
DEPARTURE = dict(nonfinite=nonfinite, tiny=tiny, tiles=tiles)
# Reduced develops: (name, source scene, box-downsampling factor, look
# scale f). depth_tie_f2_5 stands for a frame 2.5x its input's long edge:
# a non-integer f, so sigma 2.4 and the tie (4.8, 19.2).
REDUCED = (('lamp_f3', 'lamp', 3, 3.0), ('depth_tie_f3', 'depth_tie', 3, 3.0),
           ('depth_tie_f2_5', 'depth_tie', 2, 2.5))
