"""Training frames on the GPU, random patches off them, and the noise
that goes onto a patch.

A frame is what ``greycard-denoise-data export`` wrote: a mosaic in
sensor units, RGGB from its top left corner, and the target RGB the
engine's demosaic made of it, both float16, and a JSON with the frame's
own measured noise model. Every frame is memory-mapped, the native
pair and the downscaled pairs built beside it on first use, so the
process holds no frames, the set can be larger than RAM, and the page
cache holds what fits (210 frames at three scales are 62 GB). A batch
is a gather of small crops, cut by a few threads at once since a crop
the cache does not hold is a few hundred small reads.

A patch is cut at a random position and flipped or transposed. RGGB is
its own transpose; a flip keeps the pattern when the crop starts one
sample in, so the flipped crop's first column or row is again the red
one.

Noise is sampled per patch from the model the sensor has: shot noise
``a x`` with ``a`` set by the standard deviation at mid grey, read noise
``b`` as a fraction of that, both log-uniform over a range that spans
base ISO to two stops past what a full-frame camera offers (the R6 II
at ISO 10000 measures 0.016 at mid grey), the same
model across the three channels up to a little jitter. The synthetic
noise is added to the frame's own, and the sum is the model the
stabilizer is given: what a "clean" low-ISO frame carries is not zero.
"""

import glob
import json
import math
import os

import numpy as np
import torch

from vst import forward

MID_GREY = 0.18


class Frame:
    def __init__(self, stem: str, device):
        with open(stem + ".json") as f:
            self.meta = json.load(f)
        assert self.meta["pattern"] == "RGGB", stem
        # Memory-mapped: [H, W] and [H, W, 3] float16 on disk, read by
        # the crop; a downscaled frame is the same, from its cache files.
        self.stem = stem
        self.mosaic = np.load(stem + ".mosaic.npy", mmap_mode="r")
        self.target = np.load(stem + ".target.npy", mmap_mode="r")
        self.h, self.w = self.mosaic.shape
        assert self.target.shape == (self.h, self.w, 3)
        n = self.meta["noise"]
        self.a = torch.tensor(n["a"], dtype=torch.float32, device=device)
        self.b = torch.tensor(n["b"], dtype=torch.float32, device=device)
        self.name = os.path.basename(stem)
        self.scale = 1

    @property
    def pixels(self) -> int:
        return self.h * self.w

    def downscaled(self, k: int):
        """The frame at 1/k: the target RGB box-averaged over k x k and
        mosaicked again, so the pair is exact (the mosaic is that RGB
        sampled), sharper per pixel (the lens's blur shrinks by k), and
        quieter (the frame's own noise falls by k^2 in variance). This is
        where the network learns to demosaic detail a lens and a
        low-pass filter never deliver at native resolution.

        Built once and kept beside the frame as ``STEM.s{k}.mosaic.npy``
        and ``STEM.s{k}.target.npy``, memory-mapped like the native pair,
        so the process holds no frames and the page cache holds what
        fits; the in-memory copies of before left it 16 GB short and
        every batch went to disk."""
        stem = self.stem
        mosaic_path, target_path = f"{stem}.s{k}.mosaic.npy", f"{stem}.s{k}.target.npy"
        if not (os.path.exists(mosaic_path) and os.path.exists(target_path)):
            h, w = (self.h // k) & ~1, (self.w // k) & ~1
            t = torch.from_numpy(np.array(self.target[: h * k, : w * k])).permute(2, 0, 1).float()
            t = t.reshape(3, h, k, w, k).mean(dim=(2, 4))
            m = torch.empty(h, w, dtype=t.dtype)
            m[0::2, 0::2] = t[0, 0::2, 0::2]
            m[0::2, 1::2] = t[1, 0::2, 1::2]
            m[1::2, 0::2] = t[1, 1::2, 0::2]
            m[1::2, 1::2] = t[2, 1::2, 1::2]
            # Written whole under a temporary name, so a run cut short
            # leaves no half file for the next one to map.
            for path, arr in ((mosaic_path, m.half().numpy()), (target_path, t.permute(1, 2, 0).half().contiguous().numpy())):
                np.save(path + ".tmp.npy", arr)
                os.replace(path + ".tmp.npy", path)
        out = object.__new__(Frame)
        out.meta = self.meta
        out.stem = stem
        out.mosaic = np.load(mosaic_path, mmap_mode="r")
        out.target = np.load(target_path, mmap_mode="r")
        out.h, out.w = out.mosaic.shape
        assert out.target.shape == (out.h, out.w, 3)
        out.a = self.a / (k * k)
        out.b = self.b / (k * k)
        out.name = f"{self.name}@{k}"
        out.scale = k
        return out

    def crop(self, y0: int, x0: int, size: int):
        """Mosaic [S, S] and target [3, S, S] at (y0, x0), as CPU tensors."""
        m = torch.from_numpy(np.array(self.mosaic[y0 : y0 + size, x0 : x0 + size]))
        t = torch.from_numpy(np.array(self.target[y0 : y0 + size, x0 : x0 + size]))
        return m, t.permute(2, 0, 1)


def load_frames(data_dir: str, device, names=None, exclude=(), scales=(1,)):
    """Every exported frame under ``data_dir`` (one directory, or several
    separated by commas), at each of ``scales`` (1 is as exported)."""
    stems = sorted(
        p[: -len(".json")] for d in data_dir.split(",") for p in glob.glob(os.path.join(d, "*.json"))
    )
    if names is not None:
        stems = [s for s in stems if os.path.basename(s) in names]
    stems = [s for s in stems if os.path.basename(s) not in exclude]
    frames = []
    for s in stems:
        native = Frame(s, device)
        for k in scales:
            frames.append(native if k == 1 else native.downscaled(k))
    return frames


def crop_batch(frames, batch: int, size: int, generator, device, augment=True, exact_weight=1.0):
    """``batch`` patches of ``size`` x ``size`` mosaic samples and their
    targets, as float32 on ``device``, with the frames' own noise models.
    Frames are drawn by pixel count, the downscaled (exact) ones times
    ``exact_weight``.

    Returns mosaic [B, S, S], target [B, 3, S, S], a [B, 3], b [B, 3]."""
    weights = torch.tensor(
        [f.pixels * (exact_weight if f.scale > 1 else 1.0) for f in frames], dtype=torch.float64
    )
    picks = torch.multinomial(weights, batch, replacement=True, generator=generator)
    mosaics, targets, a_res, b_res = [], [], [], []
    for i in picks.tolist():
        f = frames[i]
        if augment:
            flip_h = bool(torch.randint(0, 2, (1,), generator=generator).item())
            flip_v = bool(torch.randint(0, 2, (1,), generator=generator).item())
            transpose = bool(torch.randint(0, 2, (1,), generator=generator).item())
        else:
            flip_h = flip_v = transpose = False
        # A crop that will be flipped starts one sample in, so the flip
        # brings the pattern back to RGGB.
        x0 = 2 * int(torch.randint(0, (f.w - size - 1) // 2, (1,), generator=generator).item()) + int(flip_h)
        y0 = 2 * int(torch.randint(0, (f.h - size - 1) // 2, (1,), generator=generator).item()) + int(flip_v)
        m, t = f.crop(y0, x0, size)
        if flip_h:
            m, t = m.flip(-1), t.flip(-1)
        if flip_v:
            m, t = m.flip(-2), t.flip(-2)
        if transpose:
            m, t = m.transpose(-1, -2), t.transpose(-1, -2)
        mosaics.append(m)
        targets.append(t)
        a_res.append(f.a)
        b_res.append(f.b)
    return (
        torch.stack(mosaics).to(device, non_blocking=True).float(),
        torch.stack(targets).to(device, non_blocking=True).float(),
        torch.stack(a_res).to(device),
        torch.stack(b_res).to(device),
        torch.tensor([frames[i].scale > 1 for i in picks.tolist()], device=device),
    )


def unpack_target(target: torch.Tensor) -> torch.Tensor:
    """The RGGB mosaic an RGB [B, 3, S, S] would give: the pair is exact."""
    m = torch.empty(target.shape[0], target.shape[2], target.shape[3], dtype=target.dtype, device=target.device)
    m[:, 0::2, 0::2] = target[:, 0, 0::2, 0::2]
    m[:, 0::2, 1::2] = target[:, 1, 0::2, 1::2]
    m[:, 1::2, 0::2] = target[:, 1, 1::2, 0::2]
    m[:, 1::2, 1::2] = target[:, 2, 1::2, 1::2]
    return m


def color_augment(mosaic, target, a, b, exact, generator, gain_range=(0.5, 2.0), saturation=1.5):
    """Widen the colors the network sees. Camera-space colors are far
    less saturated than the primaries of a test chart or a stage light,
    and a network that has only seen the one turns a red pepper pink.

    Every patch gets a random gain per channel (a white balance the
    camera did not have), applied to the mosaic by filter color and to
    the target, with the frame's own noise model following. A patch
    from an exact pair (a downscaled frame, whose mosaic is its target
    sampled) also gets its saturation pushed by up to ``saturation``
    about the pixel mean, and its mosaic rebuilt from the result.
    Values clip at 1 as a sensor would."""
    B = mosaic.shape[0]
    lo, hi = math.log(gain_range[0]), math.log(gain_range[1])
    g = torch.exp(lo + (hi - lo) * torch.rand(B, 3, generator=generator)).to(mosaic.device)
    s = (saturation * torch.rand(B, 1, 1, 1, generator=generator)).to(mosaic.device)
    s = s * exact[:, None, None, None]
    mean = target.mean(dim=1, keepdim=True)
    t = (mean + (1.0 + s) * (target - mean)).clamp(min=0.0)
    t = (t * g[:, :, None, None]).clamp(max=1.0)
    m_native = (mosaic * per_sample_channel(mosaic.shape, g)).clamp(max=1.0)
    m = torch.where(exact[:, None, None], unpack_target(t), m_native)
    return m, t, a * g, b * g * g


def sample_noise_model(batch: int, generator, device, sigma_range=(0.003, 0.06), read_range=(0.03, 0.6)):
    """Per patch, a shot-noise ``a`` and read-noise ``b`` per channel.

    ``sigma_range`` is the standard deviation at mid grey; ``read_range``
    the read noise's standard deviation as a fraction of it."""
    u = torch.rand(batch, 1, generator=generator).to(device)
    lo, hi = sigma_range
    sigma = lo * (hi / lo) ** u
    u = torch.rand(batch, 1, generator=generator).to(device)
    lo, hi = read_range
    read = lo * (hi / lo) ** u
    jitter = torch.exp(0.2 * (2 * torch.rand(batch, 3, generator=generator).to(device) - 1))
    a = sigma * sigma / MID_GREY * jitter
    jitter = torch.exp(0.2 * (2 * torch.rand(batch, 3, generator=generator).to(device) - 1))
    b = (read * sigma) ** 2 * jitter
    return a, b


def per_sample_channel(mosaic_shape, a: torch.Tensor):
    """Spread per-channel [B, 3] values over a mosaic [B, S, S], RGGB."""
    out = torch.empty(mosaic_shape, dtype=a.dtype, device=a.device)
    out[:, 0::2, 0::2] = a[:, 0, None, None]
    out[:, 0::2, 1::2] = a[:, 1, None, None]
    out[:, 1::2, 0::2] = a[:, 1, None, None]
    out[:, 1::2, 1::2] = a[:, 2, None, None]
    return out


def add_noise(mosaic: torch.Tensor, a: torch.Tensor, b: torch.Tensor):
    """Poisson-Gaussian noise with the model, clipped as a sensor clips."""
    a_m = per_sample_channel(mosaic.shape, a)
    b_m = per_sample_channel(mosaic.shape, b)
    a_safe = torch.clamp(a_m, min=1e-12)
    shot = a_safe * torch.poisson(mosaic / a_safe)
    shot = torch.where(a_m > 0, shot, mosaic)
    noisy = shot + torch.sqrt(b_m) * torch.randn_like(mosaic)
    return noisy.clamp(0.0, 1.0)


def pack(mosaic: torch.Tensor) -> torch.Tensor:
    """[B, S, S] RGGB mosaic to [B, 4, S/2, S/2]: R, G1, G2, B."""
    return torch.stack(
        [mosaic[:, 0::2, 0::2], mosaic[:, 0::2, 1::2], mosaic[:, 1::2, 0::2], mosaic[:, 1::2, 1::2]],
        dim=1,
    )


def stabilize_input(noisy: torch.Tensor, a: torch.Tensor, b: torch.Tensor) -> torch.Tensor:
    """Stabilize a mosaic [B, S, S] per channel and pack it."""
    a_m = per_sample_channel(noisy.shape, a)
    b_m = per_sample_channel(noisy.shape, b)
    return pack(forward(noisy, a_m, b_m))


def stabilize_target(target: torch.Tensor, a: torch.Tensor, b: torch.Tensor) -> torch.Tensor:
    """Stabilize RGB [B, 3, S, S] per channel."""
    return forward(target, a[:, :, None, None], b[:, :, None, None])


def make_batch(
    frames, batch, size, generator, device, augment=True, noise_model=None, color=True, exact_weight=1.0
):
    """Everything one step needs: stabilized packed input, stabilized
    target, the linear target, and the total noise model per patch."""
    mosaic, target, a_res, b_res, exact = crop_batch(
        frames, batch, size, generator, device, augment, exact_weight
    )
    if color:
        mosaic, target, a_res, b_res = color_augment(mosaic, target, a_res, b_res, exact, generator)
    if noise_model is None:
        a_syn, b_syn = sample_noise_model(batch, generator, device)
    else:
        a_syn, b_syn = noise_model
        a_syn = a_syn.expand(batch, 3)
        b_syn = b_syn.expand(batch, 3)
    noisy = add_noise(mosaic, a_syn, b_syn)
    a = a_syn + a_res
    b = b_syn + b_res
    x = stabilize_input(noisy, a, b)
    y = stabilize_target(target, a, b)
    return x, y, target, a, b


def psnr(a: torch.Tensor, b: torch.Tensor, peak=1.0) -> float:
    mse = torch.mean((a - b) ** 2).item()
    return 10 * math.log10(peak * peak / max(mse, 1e-12))
