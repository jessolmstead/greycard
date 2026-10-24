# §415. A four-step remove, measured

*2026-10-24*

The remove takes 25 Euler steps, about 10 s at 1024 on an RTX 5070
Ti and much longer on an Apple GPU through WebGPU. The authors of
RORem publish a faster form: "RORem-4S", not a model but their RORem
UNet with an LCM LoRA fused in, run by an LCM sampler at four steps,
guidance 1.0, strength 0.99. We built it through the same export and
measured it. It is not shipped; this records why, and what is kept.

## What it is

The LoRA (787 MB, rank 64, 788 adapters) was distilled from plain
RORem at 512, not from RORem-mixed; the paper and the training
script's defaults say so, and the fills say so below. Current
diffusers loads only 228 of its adapters, since 560 attention
adapters carry the older `lora.down/up` names and are skipped without
a word, so the authors' own script runs a partial fusion today; we
fused all 788 ourselves and found the two nearly identical at 512.
The LCM schedule from the model's betas gives timesteps 999, 759,
499, 259; strength 0.99 drops the first, so three UNet calls. Each
step predicts the clean latent and re-noises to the next timestep
with fresh noise from our generator; the sampler matches diffusers'
to 1e-5 and sits on the `rorem-4s` branch beside the Euler one,
chosen by which files the store holds, the 25-step path bit-identical.

## What it does

Parity holds against the authors' pipeline (37.4 dB at 1024 on
RORem-mixed, 33.8 at 512 on plain; floors 35 and 33). Fused onto
RORem-mixed, the base it was not trained for, it fills visibly worse:
soft texture, color mottling, hatched stripes with a blue cast on a
busy slope, the same in torch fp32, so the model and not our export.
Fused onto plain RORem, the matched base, the hatching and the cast
are gone and the texture is close; what remains is a level shift,
the fill 4 to 8 levels darker and greyer than either 25-step model on
three windows and about 20 on the slope. Plain RORem at 25 steps is
as clean as mixed and no better.

| At 512, resident | Time |
|---|---|
| RORem-mixed, 25 steps | 2.9 s |
| plain RORem, 25 steps | 3.6 s |
| plain RORem + LCM, 4 steps | 0.6 s |

At 1024 on mixed, 4 steps took 3.0 s against 10.2. One session at a
time, the loads (2.3 to 3.3 s) dominate a 4-step run.

## Decided

Mixed at 25 steps stays. Saving two to seven seconds is not worth a
darker fill, and a second 5 GB download is not worth a preview. The
branch keeps the sampler, its tests, the stores, and the scripts
(under target/rorem/4s, not in the tree). Shipping the LoRA unfused,
as low-rank branches over the shipped data file, would add 390 MB
instead of 5 GB and run 30 to 65 percent slower a step on WebGPU;
measured, not taken.

## Open

The Apple timing of a 1024 removal decides whether speed is worth
more work there. If it is, the route is our own distillation from
RORem-mixed at 1024 with the authors' training script, with the
level shift watched for; the authors' LoRA is the wrong base for it
and the wrong size.
