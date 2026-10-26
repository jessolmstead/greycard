# §429. Patch, Fast, or Best for a Fill

*2026-10-25*

The full remove makes a fill in 25 Euler steps, 10 s at 1024 on an RTX
5070 Ti and about a minute on an Apple GPU. The authors' four-step
LoRA was the wrong base for it: fills came out darker and hatched. So
we distilled our own from RORem-mixed at 1024 on the empty prompt, the
one greycard runs. On 34 held-out windows the student is as close to
the truth as the teacher is.

## The sampler

LCM over four timesteps (999, 759, 499, and 259); strength 0.99 drops
the first, so there are three UNet calls. Each call predicts the clean
latent through the boundary scalings and re-noises it to the next step
with fresh noise from the seed. The schedule and the scalings are the
scheduler's, not the LoRA's, so the student needed no change to the
sampler. Against diffusers' fp32 pipeline on the same noise the student
scores 38.2 dB at 1024, where 25 steps score 38.6.

## The choice

A Fill patch says what makes it. Patch is LaMa; Fast is four steps;
Best is 25. The sidecar keeps `fast` beside `remove`, so an older build
reads a Fast fill as Best and remakes it. The choice in hand sticks for
the next Fill and is listed in Settings. Before one is made it is Best,
or Fast where the GPU shares the machine's memory. An option that
cannot run here is greyed with its reason. The fill cache keys on the
choice.

Fast ships as low-rank branches beside the remove's own weights: 390 MB
over the remove's data file, not a second 5 GB. Its graphs name that
file, so the store puts the files in the remove's directory, with a
license note of their own. The engine holds one UNet at a time and lets
go of the other when the choice changes.

## Measured (RTX 5070 Ti, 1024)

| | warm, held | one at a time |
|---|---|---|
| Best | 10.2-10.7 s | 12.8-13.2 s |
| Fast, fused | 2.9-3.1 s | 5.4 s |
| Fast, as shipped | 3.2 s | 7.6 s |

As shipped, against the fused student, w1 at seed 1 scores 36.5 dB and
the level matches within 0.1. On large holes Fast draws shards where
Best draws shapes, at every seed we tried. Its level varies less from
seed to seed than Best's does (under 6 levels on a snow slope against
15).

## Not done

- The upload of greycard-rorem-fast, and the entry listed after it.
- The Mac timing of Fast.

## The distillation

The data was 2082 of our own frames exported headlessly at a 2400
long edge, 8000 latent pairs of random 1024 crops with LaMa-style
masks. The run followed the authors' training script: DDIM solver,
50 skeleton steps, boundary scalings at timestep scaling 10, rank 64
on their module list, one UNet with its adapters off as the teacher
at guidance 1 on the empty prompt. 4000 steps at batch 8 and lr
1e-4, then 1500 more at batch 6 and lr 2e-5 from step 3000 with an
exponential moving average of the weights (decay 0.997), because the
raw checkpoints swung five levels in a hole's mean from one to the
next while the averaged ones agreed within half a level. The
candidate is the average at step 4000. On 34 held-out windows of 17
frames outside the set, the student sits a level above the teacher
on average with a spread of two, where the teacher's own spread
against the truth is seven; against the truth both score the same.

## Review

A fresh review walked the sampler against diffusers' scheduler and
found it exact, rebuilt master to confirm the 25-step path bit for
bit, and found Fast unreachable: its model was left out of the
registry's list, so the option greyed as not downloaded with nothing
to download. It is listed now, a click on an undownloaded option
offers the download, the base model's data file is pinned by hash in
a test so a re-export cannot change Fast's weights unseen, and the
panel and Settings share one property for the choice. Open for the
user: a choice made with a patch selected also sets the default for
the next Fill, which fits "last chosen" but means one Best on a Mac
makes every later Fill a minute long.
