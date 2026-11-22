"""Trial packings of an X-Trans mosaic for the denoiser (the contract
question of the roadmap's X-Trans item; §322).

Every candidate takes the same thing at its door: the stabilized mosaic
at full resolution, one plane, at the canonical phase (the layout of
greycard-core's ``CfaPattern::xtrans()`` from its top left), and gives
RGB at full resolution in the same space, as the Bayer network does.
What differs is how the plane is packed for the trunk:

- ``A``: full resolution, the plane beside three color masks; a small
  stem at full resolution, then unshuffled by 2 into the trunk.
- ``B``: unshuffled by 6, 36 planes at 1/6, each one color.
- ``C``: unshuffled by 3, 9 planes at 1/3; the two 3x3 block types sit
  in a checkerboard, so the planes are split by type into 18 (zero on
  the other type) beside a checker plane.
- ``D``: unshuffled by 2, 4 planes at 1/2 as Bayer's, with the color
  masks unshuffled beside them (12 planes).
- ``F``: D's planes and masks with E's bilinear planes beside them:
  every sample exact, and every color at every site from the start.
- ``E``: a bilinear demosaic of the stabilized mosaic (every 3x3 of
  X-Trans holds all three colors), unshuffled by 2: denoising after a
  demosaic, the classical order.

The trunk is ``model.UNet``'s (three levels down, two 3x3 convolutions
each) and the head is v4's: features shuffled to full resolution, the
mosaic and its masks beside them, two convolutions into RGB.
"""

import math

import torch
import torch.nn as nn
import torch.nn.functional as F

from model import Block
from vst import forward

# R 0, G 1, B 2: CfaPattern::xtrans() from its origin.
XTRANS = [
    [1, 2, 1, 1, 0, 1],
    [0, 1, 0, 2, 1, 2],
    [1, 2, 1, 1, 0, 1],
    [1, 0, 1, 1, 2, 1],
    [2, 1, 2, 0, 1, 0],
    [1, 0, 1, 1, 2, 1],
]
MID_GREY = 0.18


def cfa_index(h: int, w: int, device=None) -> torch.Tensor:
    """[h, w] color index of the canonical X-Trans from (0, 0)."""
    p = torch.tensor(XTRANS, dtype=torch.long, device=device)
    return p.repeat(math.ceil(h / 6), math.ceil(w / 6))[:h, :w]


def cfa_masks(h: int, w: int, device=None) -> torch.Tensor:
    """[1, 3, h, w] one-hot color masks."""
    idx = cfa_index(h, w, device)
    return F.one_hot(idx, 3).permute(2, 0, 1)[None].float()


def periodic(x: torch.Tensor, tile: torch.Tensor) -> torch.Tensor:
    """A [C, 6, 6] tile repeated over x's [B, 1, H, W] grid, built from
    x's shape by ops every provider has (a Mul and a DepthToSpace), so
    the exported graph keeps H and W free and needs no second input."""
    c = tile.shape[0]
    seed = F.pixel_unshuffle(torch.ones_like(x), 6)[:, :1]
    return F.pixel_shuffle(seed * tile.reshape(1, c * 36, 1, 1).to(x), 6)


def mask_tile() -> torch.Tensor:
    """[3, 6, 6] one-hot colors of the canonical period."""
    return F.one_hot(torch.tensor(XTRANS), 3).permute(2, 0, 1).float()


def block_type_tile() -> torch.Tensor:
    """[1, 6, 6]: 0 on the 3x3 blocks laid as the origin's, 1 on the
    other two (R and B swapped)."""
    t = torch.zeros(1, 6, 6)
    t[:, :3, 3:] = 1
    t[:, 3:, :3] = 1
    return t


def phase_shift(transform: int):
    """The (dy, dx) at which a canonical crop of side 6k, flipped or
    transposed by ``transform`` (bit 0 flip x, bit 1 flip y, bit 2
    transpose), reads canonical again. A mirror or transpose of X-Trans
    is one of its shifts, so there always is one."""
    base = cfa_index(36, 36)
    t = apply_transform(base[None], transform)[0]
    for dy in range(6):
        for dx in range(6):
            if torch.equal(t[dy : dy + 6, dx : dx + 6], base[:6, :6]):
                return dy, dx
    raise AssertionError(transform)


def apply_transform(x: torch.Tensor, transform: int) -> torch.Tensor:
    if transform & 1:
        x = x.flip(-1)
    if transform & 2:
        x = x.flip(-2)
    if transform & 4:
        x = x.transpose(-1, -2)
    return x


SHIFTS = [phase_shift(t) for t in range(8)]


def mosaic_of(rgb: torch.Tensor) -> torch.Tensor:
    """The X-Trans mosaic [B, S, S] an RGB [B, 3, S, S] gives."""
    idx = cfa_index(rgb.shape[-2], rgb.shape[-1], rgb.device)
    return torch.gather(rgb, 1, idx[None, None].expand(rgb.shape[0], 1, -1, -1))[:, 0]


def per_site(shape, v: torch.Tensor) -> torch.Tensor:
    """Per-channel [B, 3] values spread over an X-Trans mosaic [B, S, S]."""
    idx = cfa_index(shape[-2], shape[-1], v.device)
    return v[:, idx]


def add_noise(mosaic, a, b):
    a_m, b_m = per_site(mosaic.shape, a), per_site(mosaic.shape, b)
    a_safe = torch.clamp(a_m, min=1e-12)
    shot = torch.where(a_m > 0, a_safe * torch.poisson(mosaic / a_safe), mosaic)
    return (shot + torch.sqrt(b_m) * torch.randn_like(mosaic)).clamp(0.0, 1.0)


def stabilize(mosaic, a, b):
    return forward(mosaic, per_site(mosaic.shape, a), per_site(mosaic.shape, b))


def bilinear(stab: torch.Tensor) -> torch.Tensor:
    """[B, 1, S, S] mosaic to [B, 3, S, S]: each color the mean of its
    samples in the 3x3 (every 3x3 of X-Trans holds all three)."""
    m = periodic(stab, mask_tile())
    k = torch.ones(3, 1, 3, 3, device=stab.device)
    num = F.conv2d(F.pad(stab * m, (1, 1, 1, 1), mode="reflect"), k, groups=3)
    den = F.conv2d(F.pad(m, (1, 1, 1, 1), mode="reflect"), k, groups=3)
    return num / den


PACK_SCALE = {"A": 2, "B": 6, "C": 3, "D": 2, "E": 2, "F": 2}
PACK_CIN = {"A": 64, "B": 36, "C": 19, "D": 16, "E": 12, "F": 28}
STEM = 16


class XNet(nn.Module):
    """One packing, the shared trunk and head. Input [B, 1, H, W], the
    stabilized mosaic at the canonical phase, H and W multiples of
    ``multiple()``; output [B, 3, H, W]."""

    def __init__(self, packing, widths=(48, 48, 96, 192), post=32):
        super().__init__()
        self.packing = packing
        self.s = PACK_SCALE[packing]
        self.widths = widths
        if packing == "A":
            self.stem = nn.Sequential(
                nn.Conv2d(4, STEM, 3, padding=1), nn.LeakyReLU(0.1),
                nn.Conv2d(STEM, STEM, 3, padding=1), nn.LeakyReLU(0.1),
            )
        c = PACK_CIN[packing]
        self.enc = nn.ModuleList()
        for w in widths:
            self.enc.append(Block(c, w))
            c = w
        self.dec = nn.ModuleList()
        for w in reversed(widths[:-1]):
            self.dec.append(Block(c + w, w))
            c = w
        self.head = nn.Conv2d(c, self.s * self.s * post, 3, padding=1)
        self.post = nn.Sequential(
            nn.Conv2d(post + 4, post, 3, padding=1), nn.LeakyReLU(0.1),
            nn.Conv2d(post, 3, 3, padding=1),
        )

    def multiple(self) -> int:
        return math.lcm(6, self.s * 2 ** (len(self.widths) - 1))

    def pack(self, x, masks):
        p = self.packing
        if p == "A":
            return F.pixel_unshuffle(self.stem(torch.cat([x, masks.expand(x.shape[0], -1, -1, -1)], 1)), 2)
        if p == "B":
            return F.pixel_unshuffle(x, 6)
        if p == "C":
            planes = F.pixel_unshuffle(x, 3)
            k = F.pixel_unshuffle(periodic(x, block_type_tile()), 3)[:, :1]
            return torch.cat([planes * (1 - k), planes * k, 2 * k - 1], 1)
        if p == "D":
            m = F.pixel_unshuffle(masks, 2).expand(x.shape[0], -1, -1, -1)
            return torch.cat([F.pixel_unshuffle(x, 2), m], 1)
        if p == "E":
            return F.pixel_unshuffle(bilinear(x), 2)
        if p == "F":
            m = F.pixel_unshuffle(masks, 2)
            return torch.cat([F.pixel_unshuffle(x, 2), m, F.pixel_unshuffle(bilinear(x), 2)], 1)
        raise ValueError(p)

    def forward(self, x):
        masks = periodic(x[:, :1], mask_tile())
        y = self.pack(x, masks)
        skips = []
        for i, block in enumerate(self.enc):
            y = block(y)
            if i + 1 < len(self.enc):
                skips.append(y)
                y = F.max_pool2d(y, 2)
        for block in self.dec:
            y = F.interpolate(y, scale_factor=2.0, mode="nearest")
            y = block(torch.cat([y, skips.pop()], 1))
        y = F.leaky_relu(F.pixel_shuffle(self.head(y), self.s), 0.1)
        if self.packing == "E":
            side = torch.cat([bilinear(x), torch.zeros_like(x)], 1)
        else:
            side = torch.cat([x, masks.expand(x.shape[0], -1, -1, -1)], 1)
        return self.post(torch.cat([y, side], 1))


def phase_loss(err: torch.Tensor, window=4) -> torch.Tensor:
    """train.phase_loss for the 6x6 period: the 36 phases' local error
    against all of them over the same area."""
    whole = F.avg_pool2d(err, 6 * window)
    loss = 0.0
    for dy in range(6):
        for dx in range(6):
            loss = loss + (F.avg_pool2d(err[:, :, dy::6, dx::6], window) - whole).abs().mean()
    return loss / 36


class Pool:
    """The patch pool on the device: RGB [N, 3, P, P] and the native
    mosaics [N, P, P] in float16, a kind per patch (0 native X-Trans, 1
    exact x2 X-Trans, 2 re-mosaiced Bayer) and its residual noise."""

    def __init__(self, stem, device):
        import numpy as np

        self.rgb = torch.from_numpy(np.load(stem + ".rgb.npy")).to(device)
        self.mos = torch.from_numpy(np.load(stem + ".mos.npy")).to(device)
        meta = np.load(stem + ".meta.npz")
        # A pool that keeps mosaics for its native patches only maps each
        # patch to its mosaic (-1: none, the mosaic is the target's).
        if "mos_index" in meta:
            self.mos_index = meta["mos_index"].tolist()
        else:
            self.mos_index = list(range(self.rgb.shape[0]))
        self.kind = torch.from_numpy(meta["kind"]).long().to(device)
        self.group = torch.from_numpy(meta["group"]).long().to(device)
        ab = torch.from_numpy(meta["ab"]).to(device)
        self.a, self.b = ab[:, :3].contiguous(), ab[:, 3:].contiguous()
        self.n = self.rgb.shape[0]
        self.p = self.rgb.shape[-1]

    def weights(self, mix=(0.25, 0.25, 0.5)):
        w = torch.zeros(self.n, dtype=torch.float64)
        for k, share in enumerate(mix):
            sel = (self.kind == k).cpu()
            if sel.any():
                w[sel] = share / sel.sum()
        return w


def sample_noise_model(batch, generator, device, sigma_range=(0.003, 0.06), read_range=(0.03, 0.6)):
    """data.sample_noise_model, as the Bayer runs draw it."""
    u = torch.rand(batch, 1, generator=generator).to(device)
    sigma = sigma_range[0] * (sigma_range[1] / sigma_range[0]) ** u
    u = torch.rand(batch, 1, generator=generator).to(device)
    read = read_range[0] * (read_range[1] / read_range[0]) ** u
    jitter = torch.exp(0.2 * (2 * torch.rand(batch, 3, generator=generator).to(device) - 1))
    a = sigma * sigma / MID_GREY * jitter
    jitter = torch.exp(0.2 * (2 * torch.rand(batch, 3, generator=generator).to(device) - 1))
    b = (read * sigma) ** 2 * jitter
    return a, b


def make_batch(pool, weights, batch, size, generator, color=True, device=None):
    """Stabilized mosaic [B, 1, S, S], stabilized target [B, 3, S, S],
    linear target, total a and b [B, 3], on ``device`` (the pool's by
    default: a pool can stay in host memory and send each batch)."""
    dev = device or pool.rgb.device
    picks = torch.multinomial(weights, batch, replacement=True, generator=generator).tolist()
    tr = torch.randint(0, 8, (batch,), generator=generator).tolist()
    rgbs, moss = [], []
    for i, t in zip(picks, tr):
        dy, dx = SHIFTS[t]
        room = pool.p - size - max(dy, dx)
        oy = dy + 6 * int(torch.randint(0, room // 6 + 1, (1,), generator=generator))
        ox = dx + 6 * int(torch.randint(0, room // 6 + 1, (1,), generator=generator))
        r = apply_transform(pool.rgb[i], t)[:, oy : oy + size, ox : ox + size]
        j = pool.mos_index[i]
        if j >= 0:
            m = apply_transform(pool.mos[j], t)[oy : oy + size, ox : ox + size]
        else:
            m = r.new_zeros(size, size)
        rgbs.append(r)
        moss.append(m)
    target = torch.stack(rgbs).to(dev, non_blocking=True).float()
    native_mos = torch.stack(moss).to(dev, non_blocking=True).float()
    idx = torch.tensor(picks, device=pool.kind.device)
    kind = pool.kind[idx].to(dev)
    a_res, b_res = pool.a[idx].to(dev), pool.b[idx].to(dev)
    native = (kind == 0)[:, None, None]
    exact = (kind != 0)
    if color:
        g = torch.exp(math.log(0.5) + math.log(4.0) * torch.rand(batch, 3, generator=generator)).to(dev)
        s = (1.5 * torch.rand(batch, 1, 1, 1, generator=generator)).to(dev) * exact[:, None, None, None]
        mean = target.mean(dim=1, keepdim=True)
        target = (mean + (1.0 + s) * (target - mean)).clamp(min=0.0)
        target = (target * g[:, :, None, None]).clamp(max=1.0)
        native_mos = (native_mos * per_site(native_mos.shape, g)).clamp(max=1.0)
        a_res, b_res = a_res * g, b_res * g * g
    mosaic = torch.where(native, native_mos, mosaic_of(target))
    a_syn, b_syn = sample_noise_model(batch, generator, dev)
    noisy = add_noise(mosaic, a_syn, b_syn)
    a, b = a_syn + a_res, b_syn + b_res
    x = stabilize(noisy, a, b)[:, None]
    y = forward(target, a[:, :, None, None], b[:, :, None, None])
    return x, y, target, a, b
