#!/usr/bin/env python3
"""Write the fixture the AgX port's oracle tests read, from Blender's
AgX (Eary Chow's formation): a sample of the formation LUT's own grid
nodes, and OpenColorIO's render of Blender's config over a sweep of
linear Rec.2020 colors. Run it again only when the config changes.

    target/venv-ocio/bin/python tools/agx-oracle.py

The venv is as tools/compare-transforms.py says; Blender's config and
its two formation LUTs are read from target/blender-agx (config.ocio,
luts/AgX_Base_sRGB.cube, luts/AgX_Base_Rec2020.cube; the files Blender
ships, copied there).

Three things are written. `node` lines: 200 grid nodes of the Rec.2020
formation LUT whose input lies inside Rec.2020 and within the curve's
range, as linear Rec.2020 and the 2.4-encoded value the LUT holds, to
the seven places the file prints. At a node the LUT is the formation
itself, so these hold the port to the formation to a float's rounding.
`srgb-node` lines: 200 nodes of the sRGB formation LUT inside Rec.2020
and the range and 200 outside Rec.2020 (within the range), the same
way; these hold the output rail and its weights, and measure the
departure of clipping the formation to the Rec.2020 cube before the
output matrix, which Blender's sRGB generator does not do.
`view` lines: OCIO's render on the sRGB display (the view "AgX", with
and without the look "AgX - Punchy") and on the Rec.2020 display (with
the look), from "Linear Rec.2020", over a neutral from ten stops under
mid grey to six over at a quarter stop, the Rec.2020 primaries and
secondaries, a skin, a sky and a foliage color at half a stop, and a
ring of twelve saturated Rec.2020 colors outside sRGB at a stop. Between
nodes OCIO interpolates the 57-cubed LUT tetrahedrally, and where the
formation has a corner (the clip after the outset, the sRGB rail) that
interpolation is off by whole levels; the view lines hold the port to
OCIO within that, measured and stated in the test. The sRGB view's
output is sRGB-encoded (piecewise) and the Rec.2020 view's a 2.4 power.
"""
import os
import numpy as np
import PyOpenColorIO as o

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
cfg = o.Config.CreateFromFile(f"{root}/target/blender-agx/config.ocio")
out_path = f"{root}/crates/greycard-ui/tests/fixtures/agx-oracle.txt"
MID = 0.18

# The generator's matrices (its "id65" set) from the LUT's E-Gamut log
# input to linear Rec.2020, for reading the nodes.
E_GAMUT_TO_XYZ = np.array([[0.7053968501, 0.1640413283, 0.08101774865],
                           [0.2801307241, 0.8202066415, -0.1003373656],
                           [-0.1037815116, -0.07290725703, 1.265746519]])
XYZ_TO_BT2020 = np.array([[1.7166634277958805, -0.3556733197301399, -0.2533680878902478],
                          [-0.6666738361988869, 1.6164557398246981, 0.0157682970961337],
                          [0.0176424817849772, -0.0427769763827532, 0.9422432810184308]])


def read_cube(path):
    vals, size = [], None
    for line in open(path):
        line = line.strip()
        if line.startswith("LUT_3D_SIZE"):
            size = int(line.split()[1])
        elif line and line[0] not in "#TD":
            vals.append([float(v) for v in line.split()])
    return size, np.array(vals)


def hsv_to_rgb(h, s, v):
    i = int(h * 6) % 6
    f = h * 6 - int(h * 6)
    p, q, t = v * (1 - s), v * (1 - s * f), v * (1 - s * (1 - f))
    return [(v, t, p), (q, v, p), (p, v, t), (p, q, v), (t, p, v), (v, p, q)][i]


def ocio(lin2020, display, punchy):
    t = o.DisplayViewTransform(src="Linear Rec.2020", display=display, view="AgX")
    if punchy:
        pipe = o.LegacyViewingPipeline()
        pipe.setDisplayViewTransform(t)
        pipe.setLooksOverrideEnabled(True)
        pipe.setLooksOverride("AgX - Punchy")
        proc = pipe.getProcessor(cfg)
    else:
        proc = cfg.getProcessor(t)
    img = np.ascontiguousarray(lin2020, dtype=np.float32).copy()
    proc.getDefaultCPUProcessor().applyRGB(img)
    return np.clip(img, 0, 1)


size, table = read_cube(f"{root}/target/blender-agx/luts/AgX_Base_Rec2020.cube")
_, table_srgb = read_cube(f"{root}/target/blender-agx/luts/AgX_Base_sRGB.cube")
idx = np.arange(size ** 3)
grid = np.stack([idx % size, (idx // size) % size, idx // (size * size)], -1) / (size - 1)
lin_nodes = ((MID * 2.0 ** (grid * 25 - 10)) @ E_GAMUT_TO_XYZ.T) @ XYZ_TO_BT2020.T
in_range = (lin_nodes.max(-1) <= MID * 2 ** 6.5) & (lin_nodes.max(-1) >= MID * 2 ** -10)
inside = (lin_nodes.min(-1) >= 0) & in_range
candidates = np.where(inside)[0]
pick = np.random.default_rng(7).choice(candidates, 200, replace=False)
rng = np.random.default_rng(11)
pick_srgb = np.concatenate([rng.choice(candidates, 200, replace=False),
                            rng.choice(np.where(~(lin_nodes.min(-1) >= 0) & in_range)[0], 200, replace=False)])

COLORS = [
    ("neutral", (1.0, 1.0, 1.0), 0.25),
    ("red", (1.0, 0.0, 0.0), 0.5), ("green", (0.0, 1.0, 0.0), 0.5), ("blue", (0.0, 0.0, 1.0), 0.5),
    ("cyan", (0.0, 1.0, 1.0), 0.5), ("magenta", (1.0, 0.0, 1.0), 0.5), ("yellow", (1.0, 1.0, 0.0), 0.5),
    ("skin", (0.6, 0.4, 0.3), 0.5), ("sky", (0.3, 0.5, 0.9), 0.5), ("foliage", (0.2, 0.5, 0.1), 0.5),
]
for k in range(12):
    COLORS.append((f"ring{k:02d}", hsv_to_rgb(k / 12, 0.9, 1.0), 1.0))
rows = []
for name, c, step in COLORS:
    n = int(round(16.0 / step))
    for k in range(n + 1):
        stops = -10.0 + k * step
        rows.append((name, stops, np.array(c) * MID * 2.0 ** stops))
lin = np.array([r[2] for r in rows])
srgb = ocio(lin, "sRGB", False)
srgb_punchy = ocio(lin, "sRGB", True)
rec2020_punchy = ocio(lin, "Rec.2020", True)

os.makedirs(os.path.dirname(out_path), exist_ok=True)
with open(out_path, "w") as f:
    f.write("# Blender's AgX (Eary Chow's formation), written by tools/agx-oracle.py; see its docstring.\n")
    f.write("# node <r> <g> <b> <R> <G> <B>: linear Rec.2020 in, the Rec.2020 formation LUT's 2.4-encoded value\n")
    f.write("# srgb-node <r> <g> <b> <R> <G> <B>: the same for the sRGB formation LUT, 200 nodes inside Rec.2020 and 200 outside\n")
    f.write("# view <color> <stops> <sRGB AgX rgb> <sRGB AgX Punchy rgb> <Rec.2020 AgX Punchy rgb>: OCIO's renders\n")
    for i in pick:
        f.write("node " + " ".join(f"{v:.7g}" for v in lin_nodes[i]) + " " + " ".join(f"{v:.7f}" for v in table[i]) + "\n")
    for i in pick_srgb:
        f.write("srgb-node " + " ".join(f"{v:.7g}" for v in lin_nodes[i]) + " " + " ".join(f"{v:.7f}" for v in table_srgb[i]) + "\n")
    for (name, stops, _), a, b, c in zip(rows, srgb, srgb_punchy, rec2020_punchy):
        f.write(f"view {name} {stops:+.2f} " + " ".join(f"{v:.4f}" for v in (*a, *b, *c)) + "\n")
print(f"wrote {out_path}: {len(pick)} nodes, {len(rows)} colors, {os.path.getsize(out_path)} bytes")
