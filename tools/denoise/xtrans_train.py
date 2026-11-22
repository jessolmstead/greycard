"""Train an X-Trans network: a packing of the contract trial
(``xtrans.py``), or a tier, packing D started from its Bayer twin.

    .venv/bin/python xtrans_train.py --pool POOL --packing D --out runs/x/D
    .venv/bin/python xtrans_train.py --pool POOL --train-pool train --packing D \\
        --init runs/v19/last.pt --widths 48,48,96,192 --post 32 --steps 80000 --batch 64 \\
        --patch 192 --every 2000 --vram-gib 6 --ckpt-minutes 15 --out runs/xtrans/fast

The tiers: fast as above (v19's shape); balanced from v15 (64,64,128,256,
post 64) and best from v20 (96,96,192,384, post 96), each 160000 steps
at batch 32. ``POOL`` is what ``xtrans_pool.py`` writes. A run resumes
from its ``ckpt.pt``; ``xtrans_export.py`` makes the ONNX file.

The recipe is the Bayer runs' (``train.py``): AdamW at 3e-4 with a
500-step warm-up and a cosine to 2%, bf16 autocast, gradient clip at 1,
an EMA of the weights (0.999) as the model scored, L1 in the stabilized
space plus the phase term (weight 2, here over the 6x6 period), color
augmentation, noise drawn per patch over sigma 0.003 to 0.06 at mid
grey. Validation is the held-out real X-Trans patches at four fixed
noise levels, scored as PSNR of an sRGB rendering two stops up.
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

import xtrans
from model import parameter_count
from train import rendered
from vst import forward, inverse

VAL_SIGMAS = (0.005, 0.01, 0.02, 0.04)


def psnr(a, b):
    mse = torch.mean((a - b) ** 2).item()
    return 10 * math.log10(1.0 / max(mse, 1e-12))


def val_noisy(pool, sigma, read=0.2):
    """The val patches with fixed noise at ``sigma``: stabilized input,
    the total model, and which are native."""
    g = torch.Generator(device=pool.rgb.device).manual_seed(int(sigma * 1e6))
    torch.manual_seed(int(sigma * 1e6))
    n = pool.n
    target = pool.rgb.float()
    mosaic = torch.where((pool.kind == 0)[:, None, None], pool.mos.float(), xtrans.mosaic_of(target))
    a = torch.full((n, 3), sigma * sigma / xtrans.MID_GREY, device=target.device)
    b = torch.full((n, 3), (read * sigma) ** 2, device=target.device)
    noisy = xtrans.add_noise(mosaic, a, b)
    a_t, b_t = a + pool.a, b + pool.b
    return noisy, mosaic, target, a_t, b_t


def run_model(model, noisy, a, b, chunk=8):
    outs = []
    with torch.no_grad(), torch.autocast("cuda", dtype=torch.bfloat16):
        for i in range(0, noisy.shape[0], chunk):
            x = xtrans.stabilize(noisy[i : i + chunk], a[i : i + chunk], b[i : i + chunk])[:, None]
            y = model(x).float()
            outs.append(inverse(y, a[i : i + chunk, :, None, None], b[i : i + chunk, :, None, None]).clamp(0, 1))
    return torch.cat(outs)


def validate(model, pool):
    model.eval()
    out = {}
    native = pool.kind == 0
    for sigma in VAL_SIGMAS:
        noisy, mosaic, target, a, b = val_noisy(pool, sigma)
        rgb = run_model(model, noisy, a, b)
        r_out, r_t = rendered(rgb), rendered(target)
        rec = {
            "exact": psnr(r_out[~native], r_t[~native]),
            "native": psnr(r_out[native], r_t[native]),
            "mosaic": psnr(rendered(xtrans.mosaic_of(rgb[native])), rendered(mosaic[native])),
        }
        out[sigma] = rec
    model.train()
    return out


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--pool", required=True, help="the pool's directory (train.* and val.*)")
    p.add_argument("--packing", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--steps", type=int, default=8000)
    p.add_argument("--batch", type=int, default=32)
    p.add_argument("--patch", type=int, default=192)
    p.add_argument("--lr", type=float, default=3e-4)
    p.add_argument("--widths", type=str, default="48,48,96,192")
    p.add_argument("--post", type=int, default=32)
    p.add_argument("--seed", type=int, default=0)
    p.add_argument("--every", type=int, default=500)
    p.add_argument("--phase-weight", type=float, default=2.0)
    p.add_argument("--mix", type=str, default="0.25,0.25,0.5", help="native, exact, Bayer shares")
    p.add_argument("--init", type=str, default=None, help="a Bayer checkpoint of D's shape to start from")
    p.add_argument("--train-pool", type=str, default="train", help="the training pool's name in --pool")
    p.add_argument("--host-pool", action="store_true", help="keep the training pool in host memory")
    p.add_argument("--vram-gib", type=float, default=8.0, help="the most of the card this process allocates")
    p.add_argument("--ckpt-minutes", type=float, default=10.0, help="write a resumable checkpoint this often")
    p.add_argument("--workers", type=int, default=4, help="threads cutting batches")
    args = p.parse_args()

    device = torch.device("cuda")
    torch.backends.cudnn.benchmark = True
    # The card is shared: never more than --vram-gib of it (the CUDA
    # context, a few hundred MB, is outside this).
    total = torch.cuda.get_device_properties(device).total_memory
    torch.cuda.set_per_process_memory_fraction(min(1.0, args.vram_gib * 2**30 / total), 0)
    torch.set_num_threads(4)
    torch.manual_seed(args.seed)
    os.makedirs(args.out, exist_ok=True)
    train = xtrans.Pool(os.path.join(args.pool, args.train_pool), torch.device("cpu") if args.host_pool else device)
    val = xtrans.Pool(os.path.join(args.pool, "val"), device)
    weights = train.weights(tuple(float(v) for v in args.mix.split(",")))
    widths = tuple(int(w) for w in args.widths.split(","))
    model = xtrans.XNet(args.packing, widths, args.post).to(device)
    assert args.patch % model.multiple() == 0, model.multiple()
    ckpt_path = os.path.join(args.out, "ckpt.pt")
    resume = os.path.exists(ckpt_path)
    if args.init and not resume:
        # A Bayer checkpoint of the same shape under packing D: the four
        # unshuffled planes take the Bayer first layer's weights (the
        # order R, G1, G2, B is the unshuffle's), the masks start at zero.
        assert args.packing == "D"
        state = dict(torch.load(args.init, map_location="cpu", weights_only=False)["ema"])
        own = model.state_dict()
        for key in ("enc.0.c1.weight", "post.0.weight"):
            w = torch.zeros_like(own[key])
            n = min(w.shape[1], state[key].shape[1])
            w[:, :n] = state[key][:, :n]
            if key == "post.0.weight":
                w[:, args.post + 1 :] = 0  # the Bayer pad planes; here the masks
            state[key] = w
        model.load_state_dict(state)
        print(f"initialized from {os.path.basename(os.path.dirname(args.init))}", flush=True)
    print(f"{args.packing} {widths}: {parameter_count(model) / 1e6:.2f} M parameters; "
          f"{train.n} training patches, {val.n} validation", flush=True)
    ema = copy.deepcopy(model).eval()
    for q in ema.parameters():
        q.requires_grad_(False)
    opt = torch.optim.AdamW(model.parameters(), lr=args.lr, betas=(0.9, 0.99), weight_decay=0.0)
    step0, running, before = 0, None, 0.0
    if resume:
        ck = torch.load(ckpt_path, map_location=device, weights_only=False)
        model.load_state_dict(ck["model"])
        ema.load_state_dict(ck["ema"])
        opt.load_state_dict(ck["opt"])
        step0, running, before = ck["step"], ck["running"], ck["seconds"]
        print(f"resumed at step {step0}", flush=True)

    def save_ckpt(step):
        tmp = ckpt_path + ".tmp"
        torch.save({"model": model.state_dict(), "ema": ema.state_dict(), "opt": opt.state_dict(),
                    "step": step, "running": running, "seconds": before + time.time() - start,
                    "packing": args.packing, "widths": widths, "post": args.post, "seed": args.seed}, tmp)
        os.replace(tmp, ckpt_path)

    def lr_at(step):
        if step < 500:
            return args.lr * step / 500
        t = (step - 500) / max(1, args.steps - 500)
        return args.lr * (0.02 + 0.98 * 0.5 * (1 + math.cos(math.pi * t)))

    log = open(os.path.join(args.out, "log.jsonl"), "a" if resume else "w")
    start = time.time()
    last_ckpt = start
    # Batch i is drawn from its own generator seeded by (seed, i), so
    # the stream is the same whatever the worker count and wherever a
    # run resumed; threads cut the next batches while the card steps.
    workers = max(1, args.workers)
    queues = [queue.Queue(maxsize=2) for _ in range(workers)]

    def cut(w):
        for i in range(step0 + w, args.steps, workers):
            gen = torch.Generator().manual_seed(args.seed * 1_000_003 + i)
            queues[w].put(xtrans.make_batch(train, weights, args.batch, args.patch, gen, device=device))

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
            loss = loss + args.phase_weight * xtrans.phase_loss(out - y)
        opt.zero_grad(set_to_none=True)
        loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        opt.step()
        with torch.no_grad():
            decay = min(0.999, (1 + step) / (10 + step))
            for pe, pm in zip(ema.parameters(), model.parameters()):
                pe.lerp_(pm, 1 - decay)
        running = loss.item() if running is None else 0.98 * running + 0.02 * loss.item()
        if (step + 1) % args.every == 0 or step + 1 == args.steps:
            v = validate(ema, val)
            rec = {"step": step + 1, "loss": running, "seconds": before + time.time() - start,
                   "peak_gib": torch.cuda.max_memory_allocated(device) / 2**30,
                   "peak_reserved_gib": torch.cuda.max_memory_reserved(device) / 2**30,
                   "val": {str(s): r for s, r in v.items()}}
            log.write(json.dumps(rec) + "\n")
            log.flush()
            vs = "  ".join(f"σ{s}: {r['exact']:.2f}/{r['native']:.2f}" for s, r in v.items())
            print(f"step {step + 1} loss {running:.4f} {vs} {rec['seconds'] / 60:.1f} min", flush=True)
        if time.time() - last_ckpt > 60 * args.ckpt_minutes and step + 1 < args.steps:
            save_ckpt(step + 1)
            last_ckpt = time.time()
    torch.save({"ema": ema.state_dict(), "packing": args.packing, "widths": widths, "post": args.post,
                "steps": args.steps, "seed": args.seed}, os.path.join(args.out, "last.pt"))
    save_ckpt(args.steps)


if __name__ == "__main__":
    main()
