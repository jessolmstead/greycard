# SPDX-License-Identifier: GPL-3.0-or-later
"""Write the learned highlight fill's fixtures: the random checkpoint's
weights, the Oklab matrices, and per synthetic scene its inputs and the
oracle's stages, whole frame (box (0, 0), the frame the scene's own size,
the thumbnail built from the scene).

Usage: make_fixtures.py OUT_DIR

Every array is a raw little-endian file listed in its directory's
manifest.txt (see rawio.py); `weights/` is the random checkpoint, `oklab/`
the matrices (the inverses are torch's float32 inverses: use these).
Per scene:
- inputs: x (H, W, 3), codes u8 (H, W), clips (3), box i32 (2), frame
  i32 (2), f (the look scale, shape 1); `headroom` stores no x or codes
  (they are the lamp's, x divided by its `headroom` scalar in float32);
- out_clipped (N, 3): the final picture at the N pixels with
  codes & 0x7F > 0, in row-major order (those hl-1 does not fill hold x,
  see SPEC.md); every other pixel is x, bit for bit;

Layouts: pictures and the thumbnail are channels-last (x, xh (h, w, 3),
thumb and thumb_long40 (h, w, 5)); every other multi-channel field is
channels-first, (2, h, w) for abh, harm, harm_val, harm_start, harm_eighth,
core_dab, band_dab, dab_mixed, dab_h, and dab_up, and (32, h, w) for cmap.
Half-size planes are (Hh, Wh) = (ceil(H/2), ceil(W/2)); the harmonic's
eighth-size planes (floor(Hh/4), floor(Wh/4)); the full-size fields
gain_up (H, W) and dab_up (2, H, W). Per-region rows: (n, k).
- per region, rows in ascending id order (id 0, the outside, first when
  present; its outputs are zeroed before use): ids i32, feats (n, 20),
  ctx (n, 32), outputs (n, 12, before the zeroing), params (n, 12: H, L,
  beta, gamma, kappa, C_a, C_b, beta_b, gamma_r, kappa_r, H2, L2, after
  it), depth (n), cdepth (n);
- half size, the groups GROUPS names for the scene: regions and ring (ids,
  i32), d0, dcore, dc0, harm (2, h, w), gain_h, dab_h (2, h, w);
- lamp_f3 carries every stage (FULL_KEYS), half-size inputs, harmonic
  internals at an eighth, the encoder's map, and the upsampled fields
  included; lamp carries thumb_long40, the thumbnail at a long edge of 40
  (a ragged last row and column)."""
import os
import shutil
import sys

import numpy as np
import torch

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import checkpoint  # noqa: E402
import oracle  # noqa: E402
import rawio  # noqa: E402
import scenes  # noqa: E402

# Per region, small: every scene that has a region.
REGION_KEYS = ('ids', 'feats', 'ctx', 'outputs', 'params', 'depth', 'cdepth')
# Half-size groups, chosen per scene to keep the tracked fixtures near 2 MB.
LABELS = ('regions', 'ring', 'd0', 'dcore', 'dc0')
FIELDS = ('gain_h', 'dab_h')
HARM = ('harm',)
FULL_KEYS = ('xh', 'any_share', 'core_share', 'kh', 'thumb', 'where', 'fa', 'fc', 'abh', 'yh', 'ly', 'harm_fixed',
             'harm_known', 'harm_val', 'harm_start', 'harm_eighth', 'cmap', 'inv', 'ring_owner', 'centroid', 'grid',
             'off', 'dd', 's', 'mix', 'mixg', 'gain_band', 'dc', 's2', 'tie', 'core_dab', 'band_dab', 'dab_mixed',
             'keep', 'gain_up', 'dab_up') + LABELS + FIELDS + HARM
GROUPS = {
    'lamp': ('regions', 'ring', 'dc0') + FIELDS,
    'line': ('regions', 'dcore') + FIELDS,
    'diagonal': ('regions', 'ring') + FIELDS,
    'edge_cut': ('regions', 'ring', 'dc0') + FIELDS + HARM,
    'sky': ('regions',) + FIELDS + HARM,
    'all_clipped': ('d0', 'dc0') + FIELDS + HARM,
    'no_clip': (),
    'depth_tie': (),
    'depth_tie_f2_5': ('regions', 'dc0') + FIELDS,
    'xtrans': ('regions',),
    'mixed': ('regions', 'ring', 'dcore') + FIELDS + HARM,
    'headroom': (),
    'lamp_f3': FULL_KEYS,
    'depth_tie_f3': LABELS + FIELDS + HARM,
    'nonfinite': ('regions',) + FIELDS,
    'tiny': LABELS + FIELDS + HARM,
    'tiles': FIELDS,
}
INTS = ('regions', 'ring', 'ids', 'inv', 'ring_owner', 'harm_known')


def arr(k, v):
    v = v.detach().cpu().numpy() if torch.is_tensor(v) else np.asarray(v)
    if k in INTS:
        return v.astype(np.int32)
    return v.astype(np.float32)


def scene_list():
    for name, fn in scenes.SCENES.items():
        yield name, fn(), 1.0
    for name, src, n, f in scenes.REDUCED:
        s = scenes.SCENES[src]()
        s = dict(s, x=oracle.down(s['x'], n).astype(np.float32), codes=oracle.down_codes(s['codes'], n),
                 about=f'{src} box-downsampled {n}x, its codes pooled; run look-scaled at f = {f:g}'
                       + ('' if n == f else f' (as a reduced input of a frame {f:g}x its long edge)'))
        yield name, s, float(f)
    for name, fn in scenes.DEPARTURE.items():
        yield name, fn(), 1.0


def main(out):
    torch.set_num_threads(4)
    if os.path.exists(out):
        shutil.rmtree(out)
    model = checkpoint.random_model()
    checkpoint.save(model, os.path.join(out, 'weights'))
    lab = oracle.Oklab()
    rawio.write_dir(os.path.join(out, 'oklab'), {
        'to_lms': arr('', lab.to_lms), 'lms_to_lab': arr('', lab.lab), 'from_lms': arr('', lab.from_lms),
        'from_lab': arr('', lab.from_lab), 'luma': arr('', lab.luma)})
    index = []
    for name, s, f in scene_list():
        rec = {}
        with torch.no_grad():
            pic = oracle.fill(model, s['x'], s['codes'], s['clips'], f=f, rec=rec, lab=lab)
        H, W, _ = s['x'].shape
        sel = (s['codes'] & 0x7F) > 0
        # Every pixel hl-1 does not fill is x bit for bit (NaN included).
        filled = oracle.filled(s['x'], s['codes'], rec['gain_up'], rec['dab_up'])
        y_t = lab.y(torch.from_numpy(s['x'])).numpy()
        assert np.array_equal(oracle.luma_np(s['x']).view(np.uint32), y_t.view(np.uint32))
        assert np.array_equal(pic[~filled].view(np.uint32), s['x'][~filled].view(np.uint32))
        assert np.isfinite(pic[filled]).all()
        a = {'x': s['x'].astype(np.float32), 'codes': s['codes'].astype(np.uint8),
             'clips': np.asarray(s['clips'], np.float32), 'box': np.array([0, 0], np.int32),
             'frame': np.array([W, H], np.int32), 'f': np.float32(f), 'out_clipped': pic[sel].astype(np.float32)}
        if name == 'lamp':
            # The thumbnail's box mean with a ragged last row and column.
            a['thumb_long40'] = oracle.thumbnail(s['x'], s['codes'], long=40)
        if name == 'headroom':
            # x and the codes are the lamp's, x divided by this in float32.
            del a['x'], a['codes']
            a['headroom'] = np.float32(scenes.HEADROOM)
        for k in REGION_KEYS + GROUPS[name]:
            if k in rec:
                a[k] = arr(k, rec[k])
        rawio.write_dir(os.path.join(out, name), a)
        size = sum(os.path.getsize(os.path.join(out, name, p)) for p in os.listdir(os.path.join(out, name)))
        index.append(f'{name}\t{W}x{H}\tf={f:g}\t{size}\t{s["about"]}')
        print(index[-1], flush=True)
    with open(os.path.join(out, 'scenes.txt'), 'w') as fh:
        fh.write('# name\tsize\tlook scale\tbytes\twhat it exercises\n' + '\n'.join(index) + '\n')


if __name__ == '__main__':
    main(sys.argv[1])
