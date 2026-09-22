"""Train the denoiser. See the notes, §37, for what is being learned.

    .venv/bin/python train.py --data ../../data/denoise --out runs/first

The frames sit in memory; each step cuts a batch of patches, adds
sampled noise, stabilizes, and takes an L1 step in the stabilized space.
Validation is on held-out frames at three fixed noise levels, scored as
PSNR of an sRGB rendering two stops up from sensor units (so the number
weighs shadows as a print does, as the bench's does), beside the PSNR
of the noisy input's bilinear demosaic so the number means something on
its own.
"""

import argparse
import copy
import json
import math
import os
import queue
import threading
import time

import torch
import torch.nn.functional as F

import data
from model import UNet, parameter_count
from vst import inverse


def bilinear_demosaic(mosaic: torch.Tensor) -> torch.Tensor:
    """A bilinear demosaic of an RGGB mosaic [B, S, S], for the baseline."""
    m = mosaic[:, None]
    r = torch.zeros_like(m)
    g = torch.zeros_like(m)
    b = torch.zeros_like(m)
    r[:, :, 0::2, 0::2] = m[:, :, 0::2, 0::2]
    g[:, :, 0::2, 1::2] = m[:, :, 0::2, 1::2]
    g[:, :, 1::2, 0::2] = m[:, :, 1::2, 0::2]
    b[:, :, 1::2, 1::2] = m[:, :, 1::2, 1::2]
    k_rb = torch.tensor([[1, 2, 1], [2, 4, 2], [1, 2, 1]], dtype=m.dtype, device=m.device)[None, None] / 4
    k_g = torch.tensor([[0, 1, 0], [1, 4, 1], [0, 1, 0]], dtype=m.dtype, device=m.device)[None, None] / 4
    r = F.conv2d(F.pad(r, (1, 1, 1, 1), mode="reflect"), k_rb)
    b = F.conv2d(F.pad(b, (1, 1, 1, 1), mode="reflect"), k_rb)
    g = F.conv2d(F.pad(g, (1, 1, 1, 1), mode="reflect"), k_g)
    return torch.cat([r, g, b], dim=1)


VAL_SIGMAS = (0.01, 0.03, 0.1)


def phase_loss(err: torch.Tensor, window=4) -> torch.Tensor:
    """The part of an error map [B, C, H, W] that is periodic with the
    2x2 phase: each phase's error, averaged over ``window`` x ``window``
    of its own samples, against the error of all four averaged over the
    same area. A grid is exactly the four disagreeing on a flat field;
    a wrong but smooth answer costs nothing here, since the plain L1
    already pays for it."""
    whole = F.avg_pool2d(err, 2 * window)
    loss = 0.0
    for dy in (0, 1):
        for dx in (0, 1):
            loss = loss + (F.avg_pool2d(err[:, :, dy::2, dx::2], window) - whole).abs().mean()
    return loss / 4


def rendered(linear: torch.Tensor, stops=2.0) -> torch.Tensor:
    """An sRGB encoding of sensor-unit values pushed up by ``stops``."""
    v = (linear * 2.0**stops).clamp(0.0, 1.0)
    return torch.where(v <= 0.0031308, 12.92 * v, 1.055 * v.pow(1 / 2.4) - 0.055)


def validate(model, frames, device, size=256, count=16, read=0.2):
    """PSNR per (scale, sigma): (network, bilinear baseline)."""
    model.eval()
    out = {}
    scales = sorted({f.scale for f in frames})
    with torch.no_grad():
      for scale in scales:
        val_frames = [f for f in frames if f.scale == scale]
        for sigma in VAL_SIGMAS:
            gen = torch.Generator().manual_seed(int(sigma * 1e5))
            torch.manual_seed(int(sigma * 1e5))
            a = torch.tensor([[sigma * sigma / data.MID_GREY] * 3], device=device)
            b = torch.tensor([[(read * sigma) ** 2] * 3], device=device)
            net_psnr, base_psnr = [], []
            for _ in range(count):
                mosaic, target, a_res, b_res, _ = data.crop_batch(val_frames, 1, size, gen, device, augment=False)
                noisy = data.add_noise(mosaic, a, b)
                a_t, b_t = a + a_res, b + b_res
                x = data.stabilize_input(noisy, a_t, b_t)
                y = model(x)
                rgb = inverse(y, a_t[:, :, None, None], b_t[:, :, None, None]).clamp(0, 1)
                net_psnr.append(data.psnr(rendered(rgb), rendered(target)))
                base_psnr.append(data.psnr(rendered(bilinear_demosaic(noisy)), rendered(target)))
            out[(scale, sigma)] = (sum(net_psnr) / count, sum(base_psnr) / count)
    model.train()
    return out


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--data", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--val", nargs="*", default=None, help="frame stems held out for validation")
    p.add_argument("--val-count", type=int, default=2, help="if --val is absent, hold out this many")
    p.add_argument("--steps", type=int, default=40000)
    p.add_argument("--batch", type=int, default=16)
    p.add_argument("--patch", type=int, default=256, help="mosaic samples on a side")
    p.add_argument("--lr", type=float, default=3e-4)
    p.add_argument("--widths", type=str, default="32,64,128,256")
    p.add_argument("--seed", type=int, default=0)
    p.add_argument("--resume", type=str, default=None)
    p.add_argument("--every", type=int, default=1000)
    p.add_argument("--workers", type=int, default=6, help="threads cutting batches")
    p.add_argument("--scales", type=str, default="1", help="frame scales to train on, e.g. 1,2,3")
    p.add_argument("--no-color", action="store_true", help="no color augmentation")
    p.add_argument("--head", type=str, default="shuffle", help="the head: shuffle (v0-v3) or mosaic")
    p.add_argument("--exact-weight", type=float, default=1.0, help="draw the downscaled (exact) frames this much more")
    p.add_argument(
        "--phase-weight", type=float, default=0.0,
        help="weight of a term that asks the four 2x2 phases for the same local error: against the grid",
    )
    p.add_argument(
        "--render-weight", type=float, default=0.0,
        help="weight of an L1 term on the sRGB rendering two stops up, beside the stabilized-space L1",
    )
    p.add_argument("--post", type=int, default=8, help="channels at full resolution in the head")
    args = p.parse_args()

    device = torch.device("cuda")
    torch.backends.cudnn.benchmark = True
    # The CPU's share is cutting crops; more threads than that needs
    # only fight whatever else the machine is doing.
    torch.set_num_threads(4)
    os.makedirs(args.out, exist_ok=True)

    scales = tuple(int(k) for k in args.scales.split(","))
    all_frames = data.load_frames(args.data, torch.device("cpu"), scales=scales)
    if not all_frames:
        raise SystemExit(f"no frames in {args.data}")
    if args.val is None:
        val_names = {f.name.split("@")[0] for f in all_frames[-args.val_count :]}
    else:
        val_names = set(args.val)
    base = lambda f: f.name.split("@")[0]
    train_frames = [f for f in all_frames if base(f) not in val_names]
    val_frames = [f for f in all_frames if base(f) in val_names]
    print(f"{len(train_frames)} training frames, {len(val_frames)} validation: {sorted(val_names)}")
    print(f"{sum(f.pixels for f in train_frames) / 1e6:.0f} MP of training mosaic")

    widths = tuple(int(w) for w in args.widths.split(","))
    model = UNet(widths, post=args.post, head=args.head).to(device)
    print(f"{parameter_count(model) / 1e6:.2f} M parameters")
    ema = copy.deepcopy(model).eval()
    for q in ema.parameters():
        q.requires_grad_(False)

    opt = torch.optim.AdamW(model.parameters(), lr=args.lr, betas=(0.9, 0.99), weight_decay=0.0)
    step0 = 0
    if args.resume:
        ck = torch.load(args.resume, map_location=device)
        model.load_state_dict(ck["model"])
        ema.load_state_dict(ck["ema"])
        opt.load_state_dict(ck["opt"])
        step0 = ck["step"]

    def lr_at(step):
        warm = 500
        if step < warm:
            return args.lr * step / warm
        t = (step - warm) / max(1, args.steps - warm)
        return args.lr * (0.02 + 0.98 * 0.5 * (1 + math.cos(math.pi * t)))

    log = open(os.path.join(args.out, "log.jsonl"), "a")
    start = time.time()
    running = 0.0
    # A batch is cut on the CPU while the GPU takes the last one. A crop
    # the page cache holds costs nothing; one it does not is a few
    # hundred small reads, so several threads cut at once (the copies
    # and the transfer release the GIL). Batch i is drawn from its own
    # generator seeded by (seed, i), so the stream does not depend on
    # how many workers there are or where a run resumed.
    workers = max(1, args.workers)
    queues = [queue.Queue(maxsize=2) for _ in range(workers)]

    def cut(w):
        for i in range(step0 + w, args.steps, workers):
            g = torch.Generator().manual_seed(args.seed * 1_000_003 + i)
            queues[w].put(
                data.make_batch(
                    train_frames, args.batch, args.patch, g, device,
                    color=not args.no_color, exact_weight=args.exact_weight,
                )
            )

    for w in range(workers):
        threading.Thread(target=cut, args=(w,), daemon=True).start()
    for step in range(step0, args.steps):
        for g in opt.param_groups:
            g["lr"] = lr_at(step)
        x, y, target, a, b = queues[(step - step0) % workers].get()
        with torch.autocast("cuda", dtype=torch.bfloat16):
            out = model(x)
        out = out.float()
        loss = F.l1_loss(out, y)
        if args.phase_weight > 0:
            loss = loss + args.phase_weight * phase_loss(out - y)
        if args.render_weight > 0:
            # The stabilized L1 weighs a highlight's error as a shadow's;
            # this term weighs them as a print does.
            linear = inverse(out, a[:, :, None, None], b[:, :, None, None])
            loss = loss + args.render_weight * F.l1_loss(rendered(linear), rendered(target))
        opt.zero_grad(set_to_none=True)
        loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        opt.step()
        with torch.no_grad():
            decay = min(0.999, (1 + step) / (10 + step))
            for pe, pm in zip(ema.parameters(), model.parameters()):
                pe.lerp_(pm, 1 - decay)
        running = 0.98 * running + 0.02 * loss.item() if step > step0 else loss.item()

        if (step + 1) % args.every == 0 or step + 1 == args.steps:
            val = validate(ema, val_frames, device)
            elapsed = time.time() - start
            rec = {
                "step": step + 1,
                "loss": running,
                "lr": lr_at(step),
                "val": {f"{k}@{s}": {"net": n, "bilinear": b} for (k, s), (n, b) in val.items()},
                "seconds": elapsed,
            }
            log.write(json.dumps(rec) + "\n")
            log.flush()
            vs = "  ".join(f"@{k}σ{s}: {n:.2f} ({b:.2f})" for (k, s), (n, b) in val.items())
            print(f"step {step + 1}  loss {running:.4f}  {vs}  {elapsed / 60:.1f} min", flush=True)
            torch.save(
                {"model": model.state_dict(), "ema": ema.state_dict(), "opt": opt.state_dict(),
                 "step": step + 1, "widths": widths, "head": args.head, "post": args.post},
                os.path.join(args.out, "last.pt"),
            )


if __name__ == "__main__":
    main()
