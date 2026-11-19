# SPDX-License-Identifier: GPL-3.0-or-later
"""The oracle against the trial it was cut from: both run on the synthetic
scenes, boxed as the trial runs (each scene a crop of a 6000 x 4000 frame
with a synthetic 384 x 256 thumbnail), with the fixtures' random checkpoint
and, if given, a trained one. Prints the max abs difference per stage,
then how the whole-frame path differs from the boxed one, then the
oracle's float32 against a float64 run of it (the port's tolerances).

Usage: compare_trial.py TRIAL_SCRIPTS_DIR [CHECKPOINT.pt]
(TRIAL_SCRIPTS_DIR the trial branch's scripts/highlight-learn; read only.)"""
import os
import sys

import numpy as np
import torch
import torch.nn.functional as F

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import checkpoint  # noqa: E402
import oracle  # noqa: E402
import scenes  # noqa: E402

trial_dir = sys.argv[1]
sys.path.insert(1, trial_dir)
import hl2  # noqa: E402
import hl3  # noqa: E402
import hl6  # noqa: E402
import hl_struct  # noqa: E402

torch.set_num_threads(4)
FRAME = (6000, 4000)
BOX = (2400, 1600)
MARGIN = 8
CK_FLAGS = dict(kind='struct6', band_color='rim', harm_from='unclipped', core_rise=True, thumb='384',
                core_shape='log', core_color='core', mix_sigma=6.0)


def trial_lab():
    import json
    import tempfile
    with tempfile.TemporaryDirectory(dir=os.path.join(HERE, '..', '..', 'target')) as d:
        with open(f'{d}/oklab.json', 'w') as fh:
            json.dump({'to_lms': oracle.TO_LMS, 'lms_to_lab': oracle.LMS_TO_LAB, 'luma': oracle.LUMA}, fh)
        return hl2.Oklab(f'{d}/oklab.json', 'cpu')


def trial_model(ck, f=1.0):
    """infer4.py's struct6 path with e_tied's flags, then scale9.py's look."""
    hl6.GATE = None
    m = hl6.load(ck, 'cpu')
    hl6.CORE_WHITEN, hl6.CORE_BOOST = 0.5, 1.4
    hl6.MIX_BAND_ONLY = False
    hl6.HARM_FROM = 'bright'
    hl6.DEPTH_TIE = (12.0, 48.0)
    hl6.SCALE, hl6.FEAT_FULL, hl6.AUTO_SCALE = 1.0, False, False
    hl6.MIX_SIGMA = max(6.0 / f, 1.0)
    hl6.DEPTH_TIE = (12.0 / f, 48.0 / f)
    return m


def synth_thumb():
    yy, xx = np.mgrid[0:256, 0:384].astype(np.float32)
    t = np.stack([-1.5 + np.sin(xx / 40) * np.cos(yy / 33), -1.4 + 0.8 * np.sin(yy / 25),
                  -1.6 + 0.5 * np.cos((xx + yy) / 50), np.clip(np.sin(xx / 60) * 0.5, 0, 1),
                  np.clip(np.sin(xx / 60) * 0.3, 0, 1)], -1)
    return t.astype(np.float32)


def half(x, codes):
    H, W, _ = x.shape
    ph, pw = H % 2, W % 2
    xe = np.pad(x.astype(np.float32), ((0, ph), (0, pw), (0, 0)), mode='edge')
    k = (np.pad(codes, ((0, ph), (0, pw)), mode='edge') & 0x7F)
    Hh, Wh = xe.shape[0] // 2, xe.shape[1] // 2
    q = lambda a: a.reshape(Hh, 2, Wh, 2, *a.shape[2:])
    return (q(xe).mean((1, 3)), q((k > 0).astype(np.float32)).mean((1, 3)), q((k == 3).astype(np.float32)).mean((1, 3)),
            q(k.astype(np.float32)).max((1, 3)), Hh, Wh)


def guarded(s, rec):
    """Clipped pixels hl-1 leaves as x where the trial fills them."""
    m = (s['codes'] & 0x7F) > 0
    return m & ~oracle.filled(s['x'], s['codes'], rec['gain_up'], rec['dab_up'])


def md(a, b):
    a = a.detach().numpy() if torch.is_tensor(a) else np.asarray(a)
    b = b.detach().numpy() if torch.is_tensor(b) else np.asarray(b)
    if a.shape != b.shape:
        return f'shape {a.shape} vs {b.shape}'
    return float(np.max(np.abs(a.astype(np.float64) - b.astype(np.float64)))) if a.size else 0.0


def cases():
    for name, fn in scenes.SCENES.items():
        yield name, fn(), 1.0
    for name, src, n, f in scenes.REDUCED:
        s = scenes.SCENES[src]()
        yield name, dict(s, x=oracle.down(s['x'], n).astype(np.float32), codes=oracle.down_codes(s['codes'], n)), float(f)


def against_trial(label, ck):
    print(f'== trial vs oracle, boxed, {label}')
    lab_t, lab_o = trial_lab(), oracle.Oklab()
    om = oracle.load(ck['model'])
    thumb = synth_thumb()
    worst = {}
    for name, s, f in cases():
        tm = trial_model(ck, f)
        frame = FRAME if f == 1 else (int(FRAME[0] // f), int(FRAME[1] // f))
        box = BOX if f == 1 else (int(BOX[0] // f), int(BOX[1] // f))
        with torch.no_grad():
            pt = hl6.whole(tm, lab_t, s['x'], s['codes'], s['clips'], thumb, box, frame, 'cpu')
            rec = {}
            po = oracle.fill(om, s['x'], s['codes'], s['clips'], thumb=thumb, box=box, frame=frame, f=f, rec=rec)
            # The trial's numbers on the same half-size inputs, its record on.
            xh, fa, fc, kh, Hh, Wh = half(s['x'], s['codes'])
            t = lambda a: torch.from_numpy(np.ascontiguousarray(a))
            wh = torch.tensor([[box[0] / frame[0], box[1] / frame[1], 2 * Wh / frame[0], 2 * Hh / frame[1]]])
            got = []
            gt, dt = hl6.numbers(tm, lab_t, t(xh).permute(2, 0, 1)[None], t(fa)[None, None], t(fc)[None, None],
                                 t(kh)[None, None], torch.tensor([s['clips']], dtype=torch.float32),
                                 t(thumb).permute(2, 0, 1)[None], wh, record=got)
            fam = (t(fa)[None, None] > 0.5).float()
            fcm = (t(fc)[None, None] > 0.5).float() * fam
            reg = hl_struct.labels(fam)
            row = {
                'labels': md(reg[0, 0], rec['regions']),
                'd0': md(hl3.distance(1 - fam, reach=256, step=2) * 2.0, rec['d0'][None, None]),
                'dcore': md(hl3.distance(1 - fcm, reach=8, step=2) * 2.0, rec['dcore'][None, None]),
                'ring': md(hl_struct.grow(reg, (1 - fam).to(reg.dtype), 12)[0, 0], rec['ring']),
                'harm': md(hl_struct.harmonic(rec['abh'][None], rec['harm_fixed'][None, None], 300)[0], rec['harm']),
                'cmap': md(tm.ctx(t(thumb).permute(2, 0, 1)[None])[0][0], rec['cmap']),
                'feats': md(got[0][0], rec['feats']) if got else 0.0,
                'outputs': md(got[0][1], rec['outputs']) if got else 0.0,
                'gain_h': md(gt[0, 0], rec['gain_h']),
                'dab_h': md(dt[0], rec['dab_h']),
                'final': md(pt, po),
                # Pixels that differ: hl-1's guard (Y(x) <= 1e-6) only.
                'final_px': float((pt != po).any(-1).sum()),
                'final_px_unguarded': float(((pt != po).any(-1) & ~guarded(s, rec)).sum()),
            }
        print(f'{name:14s} ' + ' '.join(f'{k}={v:.3g}' if isinstance(v, float) else f'{k}={v}' for k, v in row.items()))
        for k, v in row.items():
            if isinstance(v, float):
                worst[k] = max(worst.get(k, 0.0), v)
    print('worst', worst)


def boxed_vs_whole(ck):
    """The trial's box (the clipped pixels' bounds plus a margin, inside the
    scene) against the whole frame cropped to it; one thumbnail of the
    whole scene for both, so only the crop differs."""
    print(f'== whole frame vs boxed (oracle, same checkpoint, margin {MARGIN} px): clipped pixels in the box')
    om = oracle.load(ck['model'])
    lab = oracle.Oklab()
    for name, s, f in cases():
        if f != 1:
            continue
        x, c = s['x'], s['codes']
        H, W, _ = x.shape
        sel_all = (c & 0x7F) > 0
        if not sel_all.any():
            print(f'{name:14s} nothing clipped; the fill returns x exactly')
            continue
        ys, xs = np.nonzero(sel_all)
        y0, y1 = max(ys.min() - MARGIN, 0), min(ys.max() + 1 + MARGIN, H)
        x0, x1 = max(xs.min() - MARGIN, 0), min(xs.max() + 1 + MARGIN, W)
        thumb = oracle.thumbnail(x, c)
        with torch.no_grad():
            pw = oracle.fill(om, x, c, s['clips'])[y0:y1, x0:x1]
            pb = oracle.fill(om, x[y0:y1, x0:x1], c[y0:y1, x0:x1], s['clips'], thumb=thumb, box=(x0, y0), frame=(W, H))
        sel = sel_all[y0:y1, x0:x1]
        luma = np.asarray(oracle.LUMA, np.float32)
        ly = lambda p: np.log2(np.maximum(p[sel] @ luma, 1e-6))
        with torch.no_grad():
            ab = lambda p: lab.of(torch.from_numpy(p[sel] / np.maximum(p[sel] @ luma, 1e-6)[:, None]))[:, 1:].numpy()
        dy = np.abs(ly(pb) - ly(pw))
        dab = np.linalg.norm(ab(pb) - ab(pw), axis=1)
        same = (y0, y1, x0, x1) == (0, H, 0, W)
        print(f'{name:14s} box {x1 - x0}x{y1 - y0} at ({x0},{y0}){" = frame" if same else ""}: '
              f'|d log2 Y| median {np.median(dy):.4f} max {dy.max():.4f}; |d ab| median {np.median(dab):.5f} max {dab.max():.5f}')


def f64_run(om, s, f):
    """The oracle in float64 throughout (inputs, weights, matrices, blur
    kernels); returns its record."""
    torch.set_default_dtype(torch.float64)
    to_float = torch.Tensor.float
    torch.Tensor.float = lambda self: self.double()
    g32 = oracle.gauss
    oracle.gauss = lambda sigma, device: _gauss64(sigma)
    try:
        lab = oracle.Oklab()
        for k in ('to_lms', 'lab', 'luma'):
            setattr(lab, k, getattr(lab, k).double())
        lab.from_lms, lab.from_lab = torch.linalg.inv(lab.to_lms), torch.linalg.inv(lab.lab)
        m = oracle.load({k: v.double() for k, v in om.state_dict().items()}).double()
        x = s['x'].astype(np.float64)
        rec = {}
        H, W, _ = x.shape
        ph, pw = H % 2, W % 2
        xe = np.pad(x, ((0, ph), (0, pw), (0, 0)), mode='edge')
        k = (np.pad(s['codes'], ((0, ph), (0, pw)), mode='edge') & 0x7F)
        Hh, Wh = xe.shape[0] // 2, xe.shape[1] // 2
        q = lambda a: a.reshape(Hh, 2, Wh, 2, *a.shape[2:])
        xh = q(xe).mean((1, 3))
        fa = q((k > 0).astype(np.float64)).mean((1, 3))
        fc = q((k == 3).astype(np.float64)).mean((1, 3))
        kh = q(k.astype(np.float64)).max((1, 3))
        th = oracle.thumbnail(s['x'], s['codes']).astype(np.float64)
        t = lambda a: torch.from_numpy(np.ascontiguousarray(a))
        wh = torch.tensor([[0.0, 0.0, 2 * Wh / W, 2 * Hh / H]])
        with torch.no_grad():
            gain_h, dab_h = oracle.numbers(m, lab, t(xh).permute(2, 0, 1)[None], t(fa)[None, None], t(fc)[None, None],
                                           t(kh)[None, None], torch.tensor([s['clips']]), t(th).permute(2, 0, 1)[None],
                                           wh, f=f, rec=rec)
            g = F.interpolate(gain_h, scale_factor=2, mode='bilinear', align_corners=False)[:, :, :H, :W]
            d = F.interpolate(dab_h, scale_factor=2, mode='bilinear', align_corners=False)[:, :, :H, :W]
            mm = t(((s['codes'] & 0x7F) > 0).astype(np.float64))[None, None]
            rec['out'] = oracle.apply(lab, t(x).permute(2, 0, 1)[None], g * mm, d * mm, mm)[0].permute(1, 2, 0)
        return rec
    finally:
        oracle.gauss = g32
        torch.Tensor.float = to_float
        torch.set_default_dtype(torch.float32)


def _gauss64(sigma):
    r = int(3 * sigma)
    tt = torch.arange(-r, r + 1, dtype=torch.float64)
    k = torch.exp(-0.5 * (tt / sigma) ** 2)
    return k / k.sum()


STAGES = ('xh', 'abh', 'ly', 'regions', 'd0', 'dcore', 'dc0', 'ring', 'harm', 'cmap', 'feats', 'ctx', 'outputs',
          'dd', 's', 'mix', 'mixg', 'dc', 'cdepth', 's2', 'tie', 'gain_h', 'dab_h', 'out')


def f32_vs_f64(ck):
    print('== oracle float32 vs float64 (whole frame), max abs per stage')
    om = oracle.load(ck['model'])
    worst = {}
    for name, s, f in cases():
        r32 = {}
        with torch.no_grad():
            oracle.fill(om, s['x'], s['codes'], s['clips'], f=f, rec=r32)
        r64 = f64_run(om, s, f)
        row = {k: md(r32[k], r64[k]) for k in STAGES if k in r32 and k in r64}
        print(f'{name:14s} ' + ' '.join(f'{k}={v:.2g}' if isinstance(v, float) else f'{k}={v}' for k, v in row.items()))
        for k, v in row.items():
            if isinstance(v, float):
                worst[k] = max(worst.get(k, 0.0), v)
    print('worst', {k: float(f'{v:.3g}') for k, v in worst.items()})


if __name__ == '__main__':
    rnd = dict(CK_FLAGS, model=checkpoint.random_model().state_dict())
    against_trial('random checkpoint', rnd)
    if len(sys.argv) > 2:
        real = torch.load(sys.argv[2], map_location='cpu', weights_only=False)
        against_trial('trained checkpoint', real)
        boxed_vs_whole(real)
        f32_vs_f64(real)
    boxed_vs_whole(rnd)
    f32_vs_f64(rnd)
