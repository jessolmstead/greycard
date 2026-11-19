# SPDX-License-Identifier: GPL-3.0-or-later
"""The learned highlight fill, inference only: the reference the Rust port
is tested against.

This is the highlight-learn trial's round-six structured fill (its
`hl2.py`, `hl3.py`, `hl_struct.py`, and `hl6.py`), trimmed to the one
configuration that ships: checkpoint r6e-s1 (trained as `--model struct6
--core-rise --core-shape log --core-color core --mix-sigma 6`, pixel
constants as trained, S = 1) run with e_tied's look: whitening 0.5, core
boost 1.4, the harmonic from bright pixels, depth tie 12 to 48 px, and
no band-only hand-over. Function bodies and numerics are the trial's;
only branches for configurations that do not ship are gone, and each
stage can record what it computed.

Two things are new here and not in the trial:
- `fill(..., box=None, frame=None, thumb=None)`: box None is the whole
  frame (box (0, 0), frame the input's own size), and thumb None builds
  the thumbnail from the input as the bench's exporter does.
- `f`, the look scale of a reduced develop (the input box-downsampled f
  times against the full frame): the gain hand-over's sigma becomes
  max(6 / f, 1) and the depth tie (12 / f, 48 / f), everything else as
  trained, as the trial's `scale9.py` and `preview9.py` ("look-scaled").

Deliberate departures from the trial define the shipped fill, hl-1 (see
SPEC.md): `apply` fills a pixel only where its luminance Y(x) is finite
and above 1e-6 and its gain or offset is nonzero, and leaves every other
pixel bit for bit as x; `fill` reads non-finite channels as 0 for the
analysis (the half-size picture and the thumbnail), so a NaN or an
infinity cannot spread; at f > 1 the thumbnail is the reduced input's
own; per-region sums are float64 with integer counts; the labels run to
convergence; and under 8 px a side the harmonic is 0. Elsewhere the
oracle is the trial's to the bit.

Arrays are numpy (H, W, 3) float32 for pictures and (H, W) uint8 for the
codes (low 7 bits the count of colors clipped in the pixel's CFA cell, 0
to 3; bit 7 untrusted, ignored here). CPU, float32."""
import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F

# ---- The shipped configuration (e_tied on r6e-s1), every constant once. ----
FLOOR = 2.0 ** -12          # log2 floor on luminance (hl2.FLOOR)
ABMAX = 0.5                 # unit-luminance Oklab ab clamped to +-this at half size (hl6.ABMAX)
REACH_HALF = 256            # distance cap, half-size cells (hl_struct.REACH_HALF); 512 full px
RING = 12                   # ring width each region owns, half-size cells (hl_struct.RING)
CORE_REACH = 8              # cap of the thin-core distance, half-size cells (hl6: 8 * S)
THIN = 4.0                  # a core cell is thin within this many full px of the band (hl6: 4 * S)
OFF = 4.0                   # the rim's offset, full px, at most half the region's depth (hl6: 4 * S)
HARM_ITERS = 300            # Jacobi iterations at an eighth of full size (hl6: max(20, 300 S^2), S = 1)
HARM_BRIGHT = 0.25          # harmonic known only on unclipped cells brighter than this (HARM_FROM 'bright')
SIGMA_DEPTH = 3.0           # normalized blur of the depth fields, half-size px
SIGMA_MIX = 2.0             # the band-to-core hand-over (color, and the core's own gain)
MIX_SIGMA = 6.0             # the gain's hand-over, as trained (--mix-sigma 6); look-scaled by f
CORE_WHITEN = 0.5           # the core's ab rolled toward white with its rise (e_tied)
CORE_BOOST = 1.4            # the core rise scaled past the tie (e_tied)
DEPTH_TIE = (12.0, 48.0)    # core depth, full px, over which boost and whitening ramp in; look-scaled by f
THUMB_LONG = 384            # the thumbnail's long edge (the bench's t.npy)
STRIP = 512                 # rows a strip in the full-size apply (no effect on the result)
NOUT = 12                   # outputs a region (10 + the core rise's 2)
FEATS = ('log_area', 'log_depth', 'y_rim', 'y_one', 'y_two', 'rise', 'share_rim', 'share_one', 'share_two',
         'share_core', 'a_rim', 'b_rim', 'a_band', 'b_band', 'chroma_band', 'chroma_rim', 'thin',
         'clip_r', 'clip_g', 'clip_b')

# The working space (linear Rec.2020) to Oklab's LMS, Ottosson's LMS to
# Lab, and the luma weights: the bench's oklab.json, float32 as written.
TO_LMS = [[0.6167399883270264, 0.3602029085159302, 0.023057028651237488],
          [0.2651243209838867, 0.6358387470245361, 0.09903690218925476],
          [0.1000959500670433, 0.20388978719711304, 0.6960142850875854]]
LMS_TO_LAB = [[0.21045425534248352, 0.7936177849769592, -0.004072046838700771],
              [1.9779984951019287, -2.4285922050476074, 0.4505937099456787],
              [0.025904037058353424, 0.7827717661857605, -0.8086757659912109]]
LUMA = [0.26269999146461487, 0.6779999732971191, 0.059300001710653305]


class Oklab:
    """hl2.Oklab with the matrices inline. The inverses are torch's float32
    inverses, not core's LAB_TO_LMS: the port must use these."""

    def __init__(self, device='cpu'):
        self.to_lms = torch.tensor(TO_LMS, dtype=torch.float32, device=device)
        self.lab = torch.tensor(LMS_TO_LAB, dtype=torch.float32, device=device)
        self.from_lms = torch.linalg.inv(self.to_lms)
        self.from_lab = torch.linalg.inv(self.lab)
        self.luma = torch.tensor(LUMA, dtype=torch.float32, device=device)

    def of(self, rgb):
        lms = rgb @ self.to_lms.T
        lms = torch.sign(lms) * (lms.abs() + 1e-9) ** (1.0 / 3.0)
        return lms @ self.lab.T

    def rgb(self, lab):
        lms = lab @ self.from_lab.T
        return (lms ** 3) @ self.from_lms.T

    def y(self, rgb):
        return rgb @ self.luma


# ---- hl2 / hl3 helpers ----

def gauss(sigma, device):
    r = int(3 * sigma)
    t = torch.arange(-r, r + 1, dtype=torch.float32, device=device)
    k = torch.exp(-0.5 * (t / sigma) ** 2)
    return k / k.sum()


def blur(x, sigma):
    g = gauss(sigma, x.device)
    r = g.numel() // 2
    c = x.shape[1]
    x = F.conv2d(F.pad(x, (r, r, 0, 0), mode='replicate'), g.view(1, 1, 1, -1).expand(c, 1, 1, -1), groups=c)
    return F.conv2d(F.pad(x, (0, 0, r, r), mode='replicate'), g.view(1, 1, -1, 1).expand(c, 1, -1, 1), groups=c)


def distance(mask, reach, step=4):
    """Approximate distance in pixels (capped at `reach`) from each pixel
    to the nearest pixel where `mask` (B, 1, H, W, float) is 1."""
    d = torch.full_like(mask, float(reach))
    cur = mask
    d = torch.where(cur > 0, torch.zeros_like(d), d)
    for k in range(1, reach // step + 1):
        cur = F.max_pool2d(cur, 2 * step + 1, stride=1, padding=step)
        d = torch.where((cur > 0) & (d >= reach), torch.full_like(d, float(k * step)), d)
    return d


def unit_ab(lab, x):
    xp = x.permute(0, 2, 3, 1)
    y = lab.y(xp).clamp_min(1e-6)[..., None]
    return lab.of(xp / y)[..., 1:].permute(0, 3, 1, 2), lab.y(xp).unsqueeze(1)


class Context(nn.Module):
    """The thumbnail (5 channels: log2 fill / 4, shares clipped) to a
    feature map at a quarter of its size and a pooled vector."""

    def __init__(self, width=32):
        super().__init__()
        self.net = nn.Sequential(
            nn.Conv2d(5, 16, 3, padding=1), nn.GELU(),
            nn.Conv2d(16, 32, 3, stride=2, padding=1), nn.GELU(),
            nn.Conv2d(32, width, 3, padding=2, dilation=2), nn.GELU(),
            nn.Conv2d(width, width, 3, stride=2, padding=1), nn.GELU(),
            nn.Conv2d(width, width, 3, padding=2, dilation=2), nn.GELU())

    def forward(self, t):
        t = torch.cat([t[:, :3] / 4.0, t[:, 3:]], 1)
        f = self.net(t)
        return f, f.mean((2, 3))


class RegionNet6(nn.Module):
    """hl6.RegionNet6 with THUMB '384' and CORE_RISE: the context encoder
    and the MLP (20 features + 32 context) -> 128 -> 128 -> 12, GELU."""

    def __init__(self, ctx=32, width=128):
        super().__init__()
        self.ctx = Context(ctx)
        self.mlp = nn.Sequential(
            nn.Linear(len(FEATS) + ctx, width), nn.GELU(),
            nn.Linear(width, width), nn.GELU(),
            nn.Linear(width, NOUT))

    def outputs(self, f, ctx):
        return self.mlp(torch.cat([f, ctx], 1))


def load(state_dict):
    m = RegionNet6().eval()
    m.load_state_dict(state_dict)
    return m


# ---- hl_struct helpers ----

def labels(mask):
    """Connected components of mask (B, 1, H, W float): ids from 1, 0
    outside, by propagating the largest index (8-connected max pool,
    restricted to the mask) until nothing changes. A region's id is its
    largest flat index (row-major) plus one. hl-1 runs to convergence
    (the trial stopped at 20000 iterations)."""
    B, _, H, W = mask.shape
    # Doubles: a float's integers run out at 16 M pixels.
    mask = mask.double()
    idx = torch.arange(1, H * W + 1, device=mask.device, dtype=torch.float64).view(1, 1, H, W).expand(B, 1, H, W)
    lab = idx * mask
    i = 0
    while True:
        new = F.max_pool2d(lab, 3, stride=1, padding=1) * mask
        if i % 16 == 15 and torch.equal(new, lab):
            break
        lab = new
        i += 1
    return lab.float() if H * W < (1 << 24) else lab


def grow(lab, inside, steps):
    """Labels carried `steps` pixels out of their regions into `inside`
    (B, 1, H, W float), the ring each region owns."""
    out = lab.clone()
    for _ in range(steps):
        out = torch.where(out > 0, out, F.max_pool2d(out, 3, stride=1, padding=1) * inside)
    return out * (1 - (lab > 0).float()) * inside


def harmonic(ab, fixed, iters=300, rec=None):
    """ab (B, 2, H, W) continued into the pixels where fixed is 0 by
    Jacobi iterations at a quarter of the resolution."""
    if min(ab.shape[2:]) < 4:
        # hl-1: under 8 px on a side the quarter grid is empty (the pool
        # would raise), and the continuation is 0, as when nothing is
        # known.
        return torch.zeros_like(ab)
    q = F.avg_pool2d(ab * fixed, 4)
    fq = F.avg_pool2d(fixed, 4)
    known = fq > 0.25
    val = q / fq.clamp_min(1e-6)
    cur = torch.where(known, val, torch.zeros_like(val))
    # Start the unknowns at the mean of the known.
    mean = (val * known).sum((2, 3), keepdim=True) / known.sum((2, 3), keepdim=True).clamp_min(1)
    cur = torch.where(known, cur, mean.expand_as(cur))
    if rec is not None:
        rec['harm_known'] = known[0, 0]
        rec['harm_val'] = val[0]
        rec['harm_start'] = cur[0]
    for _ in range(iters):
        avg = F.avg_pool2d(F.pad(cur, (1, 1, 1, 1), mode='replicate'), 3, stride=1)
        cur = torch.where(known, val, avg)
    if rec is not None:
        rec['harm_eighth'] = cur[0]
    return F.interpolate(cur, size=ab.shape[2:], mode='bilinear', align_corners=False)


def apply(lab, x, gain, dab, full):
    xp = x.permute(0, 2, 3, 1)
    y_raw = lab.y(xp)
    y = y_raw.clamp_min(1e-6)[..., None]
    unit = lab.of(xp / y)
    unit = torch.cat([unit[..., :1], unit[..., 1:] + dab.permute(0, 2, 3, 1)], -1)
    rgb = lab.rgb(unit)
    yn = lab.y(rgb).clamp_min(1e-6)[..., None]
    rgb = rgb / yn * y * torch.exp2(gain.permute(0, 2, 3, 1))
    # hl-1's departures from the trial (SPEC.md):
    # - fill only where 1e-6 < Y(x) < inf. The trial divided by
    #   max(Y, 1e-6), so a clipped pixel with Y at or below 1e-6 (demosaic
    #   undershoot) came out near 1e6, and a NaN or an infinite channel came
    #   out NaN. Both now stay x, bit for bit. Y is the same x . LUMA the
    #   clamp above takes; NaN fails both comparisons.
    # - fill only where the gain or an offset is nonzero. The trial sent a
    #   clipped pixel with no field (no region near it) through the Oklab
    #   round trip, which moved it by up to about 2e-6; it now stays x.
    #   Any pixel the field reaches is filled exactly as the trial does.
    some = (gain != 0).permute(0, 2, 3, 1) | (dab != 0).any(1, keepdim=True).permute(0, 2, 3, 1)
    fill = (full.permute(0, 2, 3, 1) > 0) & (y_raw[..., None] > 1e-6) & (y_raw[..., None] < float('inf')) & some
    rgb = torch.where(fill, rgb, xp)
    return rgb.permute(0, 3, 1, 2)


def luma_np(x):
    """Y in numpy, in the order torch's `x @ LUMA` takes on the CPU:
    (r 0.2627 + g 0.678) + b 0.0593, float32, no fused multiply-add."""
    L = np.asarray(LUMA, np.float32)
    x = np.asarray(x, np.float32)
    return (x[..., 0] * L[0] + x[..., 1] * L[1]) + x[..., 2] * L[2]


def filled(x, codes, gain_up, dab_up):
    """The pixels hl-1 fills (the rest stay x): clipped, 1e-6 < Y(x) < inf,
    and a nonzero gain or offset. gain_up (H, W), dab_up (2, H, W)."""
    y = luma_np(x)
    with np.errstate(invalid='ignore'):
        return (((codes & 0x7F) > 0) & (y > 1e-6) & (y < np.inf)
                & ((gain_up != 0) | (dab_up != 0).any(0)))


# ---- hl6.numbers, the shipped branches ----

def numbers(model, lab, xh, any_share, core_share, kh, clips, thumb, where, f=1.0, rec=None):
    """The half-size gain and ab offset over every clipped cell. `f` is the
    look scale (1 = e_tied as judged). `rec`, a dict, takes every stage."""
    put = (lambda k, v: rec.__setitem__(k, v)) if rec is not None else (lambda k, v: None)
    S = 1.0
    # Look-scaled (scale9.py, preview9.py): f = 1 gives 6.0 and (12.0, 48.0) exactly.
    mix_sigma = max(MIX_SIGMA / f, 1.0)
    tie_lo, tie_hi = DEPTH_TIE[0] / f, DEPTH_TIE[1] / f
    B = xh.shape[0]
    dev = xh.device
    fa = (any_share > 0.5).float()
    fc = (core_share > 0.5).float() * fa
    band = fa * (1 - fc)
    abh, yh = unit_ab(lab, xh)
    abh = abh.clamp(-ABMAX, ABMAX)
    ly = torch.log2(yh.clamp_min(FLOOR))
    put('fa', fa[0, 0]), put('fc', fc[0, 0]), put('abh', abh[0]), put('yh', yh[0, 0]), put('ly', ly[0, 0])
    regions = labels(fa)
    reach = max(8, int(round(REACH_HALF * S)))
    d0 = distance(1 - fa, reach=reach, step=2) * 2.0
    dcore = distance(1 - fc, reach=max(2, int(round(CORE_REACH * S))), step=2) * 2.0
    dc0 = distance(1 - fc, reach=reach, step=2) * 2.0
    ring = grow(regions, (1 - fa).to(regions.dtype), max(1, int(round(RING * S))))
    put('regions', regions[0, 0]), put('d0', d0[0, 0]), put('dcore', dcore[0, 0]), put('dc0', dc0[0, 0])
    put('ring', ring[0, 0])
    sg = lambda v: max(0.5, v * S)
    cmap = model.ctx(thumb)[0]
    put('cmap', cmap[0])
    hh, ww = fa.shape[2:]
    gain_h = torch.zeros_like(fa)
    dab_h = torch.zeros_like(abh)
    harm_fixed = (1 - fa) * (yh > HARM_BRIGHT).float()
    put('harm_fixed', harm_fixed[0, 0])
    harm = harmonic(abh, harm_fixed, iters=max(20, int(round(HARM_ITERS * S * S))), rec=rec)
    put('harm', harm[0])
    yy = ((torch.arange(hh, device=dev, dtype=torch.float32) + 0.5) / hh).view(-1, 1).expand(-1, ww).reshape(-1)
    xx = ((torch.arange(ww, device=dev, dtype=torch.float32) + 0.5) / ww).view(1, -1).expand(hh, -1).reshape(-1)
    for b in range(B):
        ids, inv = torch.unique(regions[b, 0], return_inverse=True)
        n = len(ids)
        if n == 0 or (n == 1 and ids[0] == 0):
            continue
        inv = inv.view(-1)
        rv = ring[b, 0].reshape(-1)
        rinv = torch.searchsorted(ids, rv.contiguous()).clamp(max=n - 1)
        rinv = torch.where(rv > 0, rinv, torch.zeros_like(rinv))

        # hl-1's departure from the trial (SPEC.md): every per-region sum
        # is float64 and every count an integer, the mean rounded to
        # float32 once. The trial's float32 index_add_ drifted on regions
        # of a few million cells and its counts stopped at 2^24.
        def per(v, sel, idx):
            s = torch.zeros(n, dtype=torch.float64, device=dev).index_add_(0, idx, (v * sel).double())
            c = torch.zeros(n, dtype=torch.int64, device=dev).index_add_(0, idx, sel.long())
            return (s / c.clamp_min(1)).float(), c
        inside = (regions[b, 0].reshape(-1) > 0).float()
        kr = kh[b, 0].reshape(-1)
        lyb = ly[b, 0].reshape(-1)
        ab = abh[b].reshape(2, -1)
        ringm = (rv > 0).float()
        bright = ringm * (kr == 0).float() * (torch.exp2(lyb) > 0.25).float()
        b1 = inside * (kr == 1).float()
        b2 = inside * (kr == 2).float()
        bandm = band[b, 0].reshape(-1) * inside
        corem = fc[b, 0].reshape(-1) * inside
        area = torch.zeros(n, dtype=torch.int64, device=dev).index_add_(0, inv, inside.long()).float()
        depth0 = torch.zeros(n, device=dev).index_reduce_(0, inv, d0[b, 0].reshape(-1) * inside, 'amax')
        ybr, cbr = per(lyb, bright, rinv)
        yb1, cb1 = per(lyb, b1, inv)
        yb2, cb2 = per(lyb, b2, inv)
        _, ccore = per(lyb, corem, inv)
        # The counts' total as an integer, then every count as float32.
        tot = (cbr + cb1 + cb2 + ccore).clamp_min(1).float()
        cbr, cb1, cb2, ccore = cbr.float(), cb1.float(), cb2.float(), ccore.float()
        top = torch.where(cb2 > 0, yb2, torch.where(cb1 > 0, yb1, ybr))
        rise = torch.where((cbr > 0) & (cb1 + cb2 > 0), top - ybr, torch.zeros_like(top))
        ar, _ = per(ab[0], bright, rinv)
        br, _ = per(ab[1], bright, rinv)
        an, _ = per(ab[0], bandm, inv)
        bn, cbn = per(ab[1], bandm, inv)
        cbn = cbn.float()
        thin_n, _ = per((dcore[b, 0].reshape(-1) <= THIN * S).float(), corem, inv)
        fa_, fd_ = area, depth0
        feat = torch.stack([
            torch.log2(fa_.clamp_min(1)) / 16, torch.log2(fd_.clamp_min(1)) / 8,
            ybr / 4, yb1 / 4, yb2 / 4, rise, cbr / tot, cb1 / tot, cb2 / tot, ccore / tot,
            ar * 10, br * 10, an * 10, bn * 10, torch.sqrt(an ** 2 + bn ** 2) * 10, torch.sqrt(ar ** 2 + br ** 2) * 10,
            thin_n, clips[b, 0].expand(n), clips[b, 1].expand(n), clips[b, 2].expand(n)], 1)
        cx, _ = per(xx, inside, inv)
        cy, _ = per(yy, inside, inv)
        gx = where[b, 0] + where[b, 2] * cx
        gy = where[b, 1] + where[b, 3] * cy
        grid = torch.stack([gx, gy], -1).view(1, 1, n, 2) * 2 - 1
        ctx = F.grid_sample(cmap[b:b + 1], grid, mode='bilinear', padding_mode='border', align_corners=False)[0, :, 0].T
        r = model.outputs(feat, ctx)
        put('ids', ids), put('inv', inv.view(hh, ww)), put('ring_owner', rinv.view(hh, ww))
        put('feats', feat), put('centroid', torch.stack([cx, cy], 1)), put('grid', grid[0, 0]), put('ctx', ctx)
        put('outputs', r)
        if ids[0] == 0:
            r = torch.cat([torch.zeros_like(r[:1]), r[1:]], 0)
        # Zero at the rim, smoothed inside the region.
        off = torch.minimum(torch.full_like(depth0, OFF * S), 0.5 * depth0)[inv].view(1, 1, hh, ww)
        fb = fa[b:b + 1]
        dd = (d0[b:b + 1] - off).clamp(min=0.0) * fb
        dd = blur(dd, sg(SIGMA_DEPTH)) / blur(fb, sg(SIGMA_DEPTH)).clamp_min(1e-3) * fb
        depth = torch.zeros(n, device=dev).index_reduce_(0, inv, dd.reshape(-1), 'amax').clamp_min(1.0)
        H = 5.0 * torch.tanh(r[:, 0] / 5.0)
        # Clamped before the exp as well, so no input can overflow it.
        L = (depth * torch.exp(r[:, 1].clamp(-20.0, 20.0))).clamp(1.0, 4096.0)
        beta = torch.sigmoid(r[:, 2])
        gamma = torch.sigmoid(r[:, 3])
        kappa = torch.exp(0.7 * torch.tanh(r[:, 4]))
        C = 0.03 * torch.tanh(r[:, 5:7])
        beta_b = 1.5 * torch.sigmoid(r[:, 7])
        gamma_r = torch.sigmoid(r[:, 8])
        kappa_r = torch.exp(0.7 * torch.tanh(r[:, 9]))
        on = torch.ones(n, device=dev)
        grid_ = lambda v: v[inv].view(hh, ww)
        s = (1 - torch.exp(-dd[0, 0] / grid_(L))) * fb[0, 0]
        # The core's share, blurred, mixes the band's numbers into the
        # core's: no step at the band's inner edge either.
        mix = (blur(fc[b:b + 1], sg(SIGMA_MIX))[0, 0] * fb[0, 0]).clamp(0, 1)
        mix = torch.where(fc[b, 0] > 0, torch.maximum(mix, torch.full_like(mix, 0.5)), mix)
        # The wider hand-over for the gain only.
        mixg = mix
        if mix_sigma != 2.0:
            mixg = (blur(fc[b:b + 1], sg(mix_sigma))[0, 0] * fb[0, 0]).clamp(0, 1)
            mixg = torch.where(fc[b, 0] > 0, torch.maximum(mixg, torch.full_like(mixg, 0.5)), mixg)
        g = grid_(H * on) * s * (mixg + (1 - mixg) * grid_(beta_b))
        put('off', off[0, 0]), put('dd', dd[0, 0]), put('depth', depth), put('s', s), put('mix', mix), put('mixg', mixg)
        put('gain_band', g)
        fcb = fc[b:b + 1]
        dc = (dc0[b:b + 1] - off).clamp(min=0.0) * fcb
        dc = (blur(dc, sg(SIGMA_DEPTH)) / blur(fcb, sg(SIGMA_DEPTH)).clamp_min(1e-3) * fcb)[0, 0]
        cdepth = torch.zeros(n, device=dev).index_reduce_(0, inv, dc.reshape(-1), 'amax').clamp_min(1.0)
        H2 = 5.0 * torch.tanh(r[:, 10] / 5.0)
        L2 = (cdepth * torch.exp(r[:, 11].clamp(-20.0, 20.0))).clamp(1.0, 4096.0)
        # Round five's shape: still rising at the core's deepest point.
        s2 = torch.log1p(dc / grid_(L2)) / torch.log1p(grid_(cdepth) / grid_(L2))
        tie = grid_(((cdepth / S - tie_lo) / (tie_hi - tie_lo)).clamp(0, 1))
        g = g + mix * grid_(H2 * on) * s2 * (1 + (CORE_BOOST - 1) * tie)
        gain_h[b, 0] = g
        put('dc', dc), put('cdepth', cdepth), put('s2', s2), put('tie', tie)
        put('params', torch.stack([H, L, beta, gamma, kappa, C[:, 0], C[:, 1], beta_b, gamma_r, kappa_r, H2, L2], 1))
        A = abh[b]
        band_ab = torch.stack([an, bn], 0)[:, inv].view(2, hh, ww) * grid_(kappa).unsqueeze(0)
        rim_ab = torch.stack([ar, br], 0)[:, inv].view(2, hh, ww) * grid_(kappa_r).unsqueeze(0)
        has_band = grid_((cbn > 0).float()).unsqueeze(0)
        has_rim = grid_((cbr > 0).float()).unsqueeze(0)
        core_dab = (grid_(beta).unsqueeze(0) * (harm[b] - A)
                    + has_band * grid_(gamma).unsqueeze(0) * (band_ab - A)
                    + C[inv].T.reshape(2, hh, ww) * s)
        band_dab = has_rim * grid_(gamma_r).unsqueeze(0) * (rim_ab - A) * s
        # 'core': a core cell takes the core's color whole.
        cmix = torch.where(fc[b, 0] > 0, torch.ones_like(mix), mix)
        dab_h[b] = (core_dab * cmix + band_dab * (1 - cmix)) * grid_(on).unsqueeze(0) * fb[0]
        put('core_dab', core_dab), put('band_dab', band_dab), put('dab_mixed', dab_h[b].clone())
        # The core's color rolls toward white with its own rise: the
        # fill's ab (A + dab) times 1 - w s2, zero at the core's edge.
        keep = 1 - CORE_WHITEN * mix * s2 * grid_(on) * tie
        dab_h[b] = ((A + dab_h[b]) * keep.unsqueeze(0) - A) * fb[0]
        put('keep', keep)
    put('gain_h', gain_h[0, 0]), put('dab_h', dab_h[0])
    return gain_h, dab_h


# ---- The thumbnail and the whole-frame entry point ----

def thumbnail(x, codes, long=THUMB_LONG):
    """The frame box-averaged to `long` on its long side (the bench's
    `thumbnail`, highlight_data.rs): per cell the log2 of the mean of x
    (each channel's negatives taken as 0, floor 1/4096), the share of
    pixels with anything clipped, and the share fully clipped. f64 sums.
    The factor is an integer, ceil(long edge / long), at least 1; the last
    row and column of cells may cover fewer pixels."""
    h, w = codes.shape
    f = max(-(-max(w, h) // long), 1)
    th, tw = -(-h // f), -(-w // f)
    k = codes & 0x7F
    out = np.empty((th, tw, 5), np.float32)
    xd = np.maximum(x.astype(np.float64), 0.0)
    for ty in range(th):
        for tx in range(tw):
            ys, xs = slice(ty * f, min((ty + 1) * f, h)), slice(tx * f, min((tx + 1) * f, w))
            blk = xd[ys, xs].reshape(-1, 3)
            n = float(blk.shape[0])
            # Sequential f64 sums in row-major order, as the Rust does.
            acc = [0.0, 0.0, 0.0]
            for p in blk:
                acc[0] += p[0]
                acc[1] += p[1]
                acc[2] += p[2]
            kb = k[ys, xs].reshape(-1)
            for c in range(3):
                out[ty, tx, c] = np.log2(max(np.float32(acc[c] / n), np.float32(1.0 / 4096.0)))
            out[ty, tx, 3] = np.float32(float((kb > 0).sum()) / n)
            out[ty, tx, 4] = np.float32(float((kb == 3).sum()) / n)
    return out


def down(a, f):
    """A reduced develop's input: box-downsampled f times, the ragged
    edge dropped (scale9.py, preview9.py)."""
    h, w_ = a.shape[0] // f * f, a.shape[1] // f * f
    a = a[:h, :w_]
    return a.reshape(h // f, f, w_ // f, f, *a.shape[2:]).mean((1, 3))


def down_codes(k, f):
    """Its codes: 3 where more than half the block is fully clipped, else
    the block's largest count capped at 2, 0 where nothing is clipped
    (scale9.py, preview9.py; their input is codes & 0x7F)."""
    k = k & 0x7F
    h, w_ = k.shape[0] // f * f, k.shape[1] // f * f
    k = k[:h, :w_].reshape(h // f, f, w_ // f, f)
    core = (k == 3).mean((1, 3)) > 0.5
    mx = k.max((1, 3))
    return np.where(core, 3, np.where(mx > 0, np.minimum(np.maximum(mx, 1), 2), 0)).astype(np.uint8)


def fill(model, x, codes, clips, thumb=None, box=None, frame=None, f=1.0, rec=None, lab=None, device='cpu',
         strip=STRIP):
    """hl6.whole: the numbers at half size, the picture in strips.

    box None (with frame None) is the whole frame: box (0, 0) and the
    frame the input's own size, as the editor runs it. thumb None builds
    it from the input (the whole frame's thumbnail; only right when the
    input is the whole frame). `f` the look scale (see the module's
    docstring). Returns the filled picture (H, W, 3) float32."""
    lab = lab or Oklab(device)
    H, W, _ = x.shape
    if box is None:
        box = (0, 0)
    if frame is None:
        frame = (W, H)
    # hl-1's departure from the trial: the analysis reads a non-finite
    # channel as 0, so one NaN or infinity cannot reach a region's means
    # and every pixel of it. Finite inputs are untouched.
    xa = np.where(np.isfinite(x), x, 0).astype(np.float32)
    if thumb is None:
        thumb = thumbnail(xa, codes)
    put = (lambda k, v: rec.__setitem__(k, v)) if rec is not None else (lambda k, v: None)
    ph, pw = H % 2, W % 2
    xe = np.pad(xa, ((0, ph), (0, pw), (0, 0)), mode='edge')
    k = (np.pad(codes, ((0, ph), (0, pw)), mode='edge') & 0x7F)
    Hh, Wh = xe.shape[0] // 2, xe.shape[1] // 2
    q = lambda a: a.reshape(Hh, 2, Wh, 2, *a.shape[2:])
    xh = q(xe).mean((1, 3))
    fa = q((k > 0).astype(np.float32)).mean((1, 3))
    fc = q((k == 3).astype(np.float32)).mean((1, 3))
    kh = q(k.astype(np.float32)).max((1, 3))
    del xe
    put('thumb', thumb), put('xh', xh), put('any_share', fa), put('core_share', fc), put('kh', kh)
    t = lambda a: torch.from_numpy(np.ascontiguousarray(a)).to(device)
    cl = torch.tensor([clips], dtype=torch.float32, device=device)
    th = t(thumb.astype(np.float32)).permute(2, 0, 1)[None]
    fw, fh = frame
    wh = torch.tensor([[box[0] / fw, box[1] / fh, 2 * Wh / fw, 2 * Hh / fh]], device=device)
    put('where', wh[0])
    with torch.no_grad():
        gain_h, dab_h = numbers(model, lab, t(xh).permute(2, 0, 1)[None], t(fa)[None, None], t(fc)[None, None],
                                t(kh)[None, None], cl, th, wh, f=f, rec=rec)
        out = np.empty((H, W, 3), np.float32)
        if rec is not None:
            rec['gain_up'] = np.empty((H, W), np.float32)
            rec['dab_up'] = np.empty((2, H, W), np.float32)
        for r0 in range(0, H, strip):
            r1 = min(r0 + strip, H)
            h0, h1 = max(r0 // 2 - 1, 0), min((r1 + 1) // 2 + 1, Hh)
            g = F.interpolate(gain_h[:, :, h0:h1], scale_factor=2, mode='bilinear', align_corners=False)
            d = F.interpolate(dab_h[:, :, h0:h1], scale_factor=2, mode='bilinear', align_corners=False)
            g, d = g[:, :, r0 - 2 * h0:r1 - 2 * h0, :W], d[:, :, r0 - 2 * h0:r1 - 2 * h0, :W]
            if rec is not None:
                rec['gain_up'][r0:r1] = g[0, 0].numpy()
                rec['dab_up'][:, r0:r1] = d[0].numpy()
            m = t(((codes[r0:r1] & 0x7F) > 0).astype(np.float32))[None, None]
            xs = t(x[r0:r1].astype(np.float32)).permute(2, 0, 1)[None]
            out[r0:r1] = apply(lab, xs, g * m, d * m, m)[0].permute(1, 2, 0).cpu().numpy()
    put('out', out)
    return out
