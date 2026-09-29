# 37. The learned denoiser: data, network, training, and where it runs (2026-09-07)

The plan in §34 is now built as far as a first model. What was
decided on the way, since each choice constrains the next:

### The domain the network works in

The network sees and predicts the variance-stabilized space of §13g:
each channel through `2x / (sqrt(a x + s0^2) + s0)` with the frame's
own noise model, which makes the noise unit white whatever the ISO or
the camera, so one network serves every frame and never has to be
told the noise level. That transform is gain-invariant: white balance
scales `x` by `g`, `a` by `g` and `b` by `g^2`, and the value comes
out the same, so training in sensor units (no white balance) and
running on the balanced mosaic with the model transformed by
`NoiseModel::after_gains` is the same computation. The only thing the
gains move is where a channel clips, which the network never needs to
know.

Its output is RGB in the same space, per channel, and comes back to
linear through the *algebraic* inverse `a d^2 / 4 + d s0`
(`Vst::inverse_clean`, now public beside the unbiased inverse), which
is the exact inverse of the forward: the network's output estimates
the transform of the clean signal, and the unbiased inverse of
Mäkitalo and Foi is for the transform of a noisy sample, which this
is not. `tools/denoise/vst.py` carries the same two closed forms;
both sides have a round-trip test.

### The data

`greycard-denoise-data` (a second binary in the bench crate) has a
`survey` that reads the metadata of every raw under a folder, so a
set can be chosen by camera and ISO without decoding, and an `export`
that writes a frame as the trainer reads it: the mosaic in sensor
units cut to an RGGB origin with even sides, the target the engine's
default demosaic (AMaZE-VNG4 with the noise-model threshold) made of
it and divided back by the gains, both float16 `.npy`, and a JSON
with the frame's own measured noise model. The archive on the file
server (8250 raws; a CIFS mount, so a survey of all of it reads 240
GB, which is why the survey was run on a spread sample of 448) gave 79
frames at ISO 200 or below across the R6 II, R5 II, R8, the GFX 100S
II at ISO 80 and one DJI DNG: 2.7 gigapixels of mosaic, 21 GB on
disk, which is why the trainer keeps frames in system memory and sends
crops to the GPU rather than the reverse.

**The target is not clean.** A "clean" frame at ISO 100 measures a
sigma of 0.002 to 0.004 at mid grey, and the DJI's small sensor
0.0075. Rather than pretend otherwise, the trainer adds the frame's
own model to the noise it synthesizes, and that sum is the model the
stabilizer is given, so the network is told the truth about its
input. What it cannot be told is that the target carries the same
residual, which puts a floor under how clean the output can be at the
lowest synthetic noise; the sampling range starts at 0.004, where the
residual is already a minority.

The noise put on is the model's own: shot noise as a true Poisson
draw scaled by `a`, read noise Gaussian with `b`, clipped to 0..1 as
`normalize_levels` clips, with `a` from a sigma at mid grey drawn
log-uniformly over 0.004 to 0.12 and `b` from a read fraction over
0.03 to 0.6 of it, and a fifth of a stop of per-channel jitter. The
upper end is about two stops past what a full-frame camera reaches
(the R6 II at ISO 25600 is about 0.03 by the full well; the DNG at
ISO 1000 in the test folder measures 0.0073).

Patches are 256 mosaic samples on a side, flipped and transposed:
RGGB is its own transpose, and a flipped crop starts one sample in so
the flip puts the red back at the origin.

### The network

A UNet on the packed mosaic (R, G1, G2, B at half resolution, 4
channels in), widths 32/64/128/256 with two 3x3 convolutions and a
leaky ReLU each, max pool down, nearest up with skips, and a head to
12 channels shuffled into RGB at the mosaic's resolution. 1.95 M
parameters; the reach of the bottom level is about 90 mosaic samples
either way. Every op is a plain ONNX op (Conv, LeakyRelu, MaxPool,
Resize, Concat, DepthToSpace); the export uses opset 17 with free
height and width, and `export.py` checks it against PyTorch on random
input before writing the hash.

Training: AdamW at 3e-4 with a 500-step warm-up and a cosine down to
2%, batch 32, bf16 autocast, gradient clip at 1, an EMA of the
weights (decay 0.999) as the model that ships, L1 in the stabilized
space. 30 000 steps take under half an hour on the 5070 Ti. Validation
holds out three frames (an R6 II, an R5 II, an R8) and scores PSNR of
an sRGB rendering two stops up at sigma 0.01, 0.03 and 0.1, beside a
bilinear demosaic of the noisy input so the number has a floor.

### Where it runs

`greycard_ai::Denoiser` loads the ONNX on the first provider that
runs it and takes the balanced mosaic, its pattern and the transformed
model, as the engine's demosaic would. Any 2x2 Bayer phase is
presented as RGGB by starting tiles where the pattern reads that way;
edges are mirrored about the edge sample (`-1` reads `1`), which keeps
the phase, where a mirror about the edge would not. Tiles are 1536
with a 96 margin. Its tests run a fixed one-layer network
(`tests/fixtures/replicate.onnx`, made by `export.py --fixture`)
whose answer is known, on every phase, an odd size, and small tiles
against one.

Core's `develop` is now three public steps, `prepare`,
`demosaic_prepared` and `finish`, so a consumer can put the network
between the first and the last: the CLI's `--ai-denoise MODEL` and
the bench's `--ai-model MODEL` do exactly that. Core still never sees
the runtime.

### What the first model taught (2026-09-07)

v0 (30 000 steps, sigma sampled 0.004 to 0.12) on the bench, linear
domain, against the hybrid of §13r at the same noise; the network's
own noise estimate in both cases:

| noise | set   | none  | hybrid | ai v0 |
|-------|-------|------:|-------:|------:|
| 0.02  | Kodak | 30.86 | 32.59  | 29.67 |
| 0.02  | McM   | 29.26 | 31.34  | 27.90 |
| 0.05  | Kodak | 24.22 | 29.55  | 28.36 |
| 0.05  | McM   | 24.12 | 27.97  | 26.88 |
| 0.1   | Kodak | 18.26 | 25.55  | 26.54 |
| 0.1   | McM   | 19.01 | 23.38  | 23.90 |

Three things had to be found first:

- **Values below black.** The bench's noise is unclamped; the
  network trained on what `normalize_levels` makes, which is clamped
  at zero, and a negative sample in the stabilized space put a dark
  speckle in the output. At 0.1 that cost 4.5 dB on kodim19 alone.
  `Denoiser::run` now reads anything below zero as zero, which is the
  contract the data has; real frames never trip it.
- **The estimator is not the gap.** `--true-model` on the bench hands
  both denoisers the noise that was put on instead of the estimate
  (which reads 30 to 40 percent high on the references' own grain):
  the network gains nothing from it.
- **The ceiling is the demosaic, not the denoise.** At noise 0.005
  the network scores 29.7 dB on Kodak, which is bilinear (29.75),
  where AMaZE-VNG4 has 35.1. Its error against the reference is
  better than the hybrid's below a few cycles per pixel and worse
  above. The targets are why: a raw through a lens and a low-pass
  filter has no pixel-level detail to learn from, so the network
  never learned to resolve any, and on film scans that shows. The
  training set now also carries every frame box-downscaled by 2 and
  by 3 and mosaicked again: an exact pair (the mosaic is that RGB
  sampled), sharper per pixel, and quieter by the square of the
  factor. The frame's own noise is scaled with it.

The other suspect, a 2x2 pattern from the sub-pixel head (the zipper
score sits near 18 percent at every noise level, against the hybrid's
9 to 15), gets a small convolution after the shuffle at the mosaic's
resolution, and the noise range is brought down to 0.003 to 0.06,
since an R6 II at ISO 10000 measures 0.016 at mid grey and the top
end was two stops past anything real. On the ISO 10000 portrait
itself v0 already reads as well as the hybrid: hair strands kept,
the skin clean, no blotches.

### v1, and what McMaster's peppers said (2026-09-07)

v1 (the post-shuffle convolution, sigma 0.003 to 0.06, 60 000 steps)
against v0 on the bench, CPSNR:

| noise | set   | hybrid | v0    | v1    |
|-------|-------|-------:|------:|------:|
| 0.02  | Kodak | 32.59  | 29.67 | 31.21 |
| 0.02  | McM   | 31.34  | 27.90 | 27.76 |
| 0.05  | Kodak | 29.55  | 28.36 | 29.14 |
| 0.05  | McM   | 27.97  | 26.88 | 26.59 |
| 0.1   | Kodak | 25.55  | 26.54 | 26.85 |
| 0.1   | McM   | 23.38  | 23.90 | 24.34 |

Kodak moved up 1.5 dB at 0.02 and McMaster did not, and the crops
say why. On the red pepper of McMaster 11, v0 holds the color under
a plain 2x2 checkerboard, which the head was for, and v1 has no
checkerboard and turns the pepper pink: its blue channel scores 20
dB. The bench feeds sRGB primaries as if they were camera space, and
camera space is nowhere near that saturated (a red pepper before the
matrix is R 0.6, G 0.3, B 0.15, not 0.6, 0.05, 0.03), so the network
has never seen such a color; v1's last convolution mixes the
channels and learned a prior across them that fails there. A stage
light, an LED or a flower can get close enough on a real file for
this to matter, and a denoiser that recolors is not one.

So the trainer now augments color: every patch gets a random gain
per channel between 0.5 and 2 (a white balance the camera did not
have, applied to the mosaic by filter color and to the target, the
frame's own noise model following), and a patch from an exact pair
(a downscaled frame) has its saturation pushed by up to 1.5 about
the pixel mean and its mosaic rebuilt from the result. Native pairs
cannot be recolored beyond gains, since the mosaic is the sensor's
and a matrix does not apply to a single sample. On the ISO 10000
portrait v0 and v1 read alike: cleaner than the hybrid in the skin,
strands and pores kept.

### The bench in camera space (2026-09-07)

The bench mosaicked sRGB primaries as if a sensor saw them, which
no sensor does: a camera's channels overlap, so its space is much
less saturated than sRGB's, and a learned model trained on camera
data was being asked about colors it cannot meet on a real file.
`--camera-space` (needs `--linear`) now takes the reference through
a Canon R6 Mark II's D65 `ColorMatrix` and the gains that put white at
1, 1, 1, with a common scale so nothing exceeds the clip, mosaics
that, and inverts the matrix after the demosaic, so the algorithms see
what the engine's demosaic sees. The hybrid is not indifferent to it
either: its McMaster score at 0.02 drops 0.4 dB in camera space,
where the chroma channels are lower against the same noise. CPSNR:

| noise | set   | none  | hybrid | v1    | v2    |
|-------|-------|------:|-------:|------:|------:|
| 0.02  | Kodak | 29.21 | 31.85  | 30.20 | 30.23 |
| 0.02  | McM   | 28.11 | 30.94  | 29.12 | 28.78 |
| 0.05  | Kodak | 22.45 | 28.94  | 28.22 | 28.41 |
| 0.05  | McM   | 22.47 | 27.51  | 27.23 | 27.07 |
| 0.1   | Kodak | 16.68 | 25.25  | 26.24 | 26.26 |
| 0.1   | McM   | 17.34 | 22.83  | 24.61 | 24.51 |

So: the network is 1.6 to 1.8 dB behind the hybrid at 0.02, half a
dB behind at 0.05, and 1 to 1.8 dB ahead at 0.1, with half the
hybrid's coarse error there (1.06 against 2.03 on Kodak: blotches
are what the hybrid leaves and the network does not) and a zipper
score still twice the hybrid's at every level, so a fine periodic
residue remains to be found. The exact-truth downscaled pairs (v2)
did not move the low-noise number, which means the ceiling there is
not for want of sharp targets; the next suspects are the head's
capacity at full resolution and the loss, which in the stabilized
space weighs a highlight's error as a shadow's. The crossover near
0.05 is beyond any of the archive's cameras (ISO 10000 on the R6 II
measures 0.016), which is why the real ISO 10000 portrait reads
cleaner from the network than from the hybrid while the bench
prefers the hybrid at that noise: PSNR pays for the fine texture the
hybrid keeps, and the eye pays for the blotches it leaves.

### v3: color augmentation, and where the first day ends (2026-09-07)

v3 is v2 with the color augmentation above, batch 64, 60 000 steps.
CPSNR, the bench in camera space (the sRGB-space numbers move the same
way):

| noise | set   | hybrid | v1    | v3    |
|-------|-------|-------:|------:|------:|
| 0.02  | Kodak | 31.85  | 30.20 | 30.10 |
| 0.02  | McM   | 30.94  | 29.12 | 29.55 |
| 0.05  | Kodak | 28.94  | 28.22 | 28.37 |
| 0.05  | McM   | 27.51  | 27.23 | 27.65 |
| 0.1   | Kodak | 25.25  | 26.24 | 26.43 |
| 0.1   | McM   | 22.83  | 24.61 | 25.31 |

In sRGB space, where the colors are the extreme ones, McMaster moves
from 27.76 to 29.49 at 0.02 and from 24.34 to 25.67 at 0.1: the
pepper is red again, to the eye as to the numbers. So the state at the
end of the first day: the network is level with the hybrid at 0.05 and
ahead by 1.2 to 2.5 dB at 0.1, with half its blotch error, and behind
by 1.4 to 1.8 dB at 0.02, where the demosaic dominates. On the real
ISO 10000 portrait every model since v0 reads cleaner than the hybrid.

What is left, in order of what it would move:

1. **The 2x2 residue.** On the flat red of the pepper v3 shows a faint
   grid that v1 did not; the zipper score has said so at every noise
   level (25 to 30 percent against the hybrid's 10 to 16). The
   post-shuffle convolution is not enough on its own. Candidates: a
   head that predicts at half resolution and adds a full-resolution
   residual through a fixed bilinear up-sample, a wider post
   convolution, or a term in the loss against the 2x2 phase means.
2. **Demosaic fidelity at low noise.** The 1.5 dB at 0.02 is the
   difference between bilinear-grade and AMaZE-grade detail. The exact
   pairs did not close it, so it is the head's capacity at full
   resolution, or the L1 in the stabilized space weighting highlight
   detail no more than shadow noise; a small linear-domain term, or
   more width in the first level, are the next runs.
3. **Data.** 79 frames from one photographer's archive fit the
   network but do not cover it: foliage, fabric, text, night and
   saturated color are thin, and the pepper showed what that costs.
   Content, not cameras, is what to add; the stabilizer takes the
   cameras out.
4. **Speed.** A step is a third to a half Python cutting crops. A
   patch pool on the GPU with a vectorized gather is the fix, and
   lifts the batch for free.

Then the plumbing the plan (§34) already names: published weights
with a registry entry, the editor's stage between `prepare` and
`finish`, the DNG cache, and the strength blend.

### v4: the head sees the mosaic (2026-09-08)

The 2x2 residue and the low-noise gap have the same address: the
head. In v0 to v3 it was one convolution into 4 x 8 channels, a
shuffle to full resolution, and one 3 x 3 convolution into RGB, so
every output pixel's value came through filters of its own phase,
and nothing at full resolution could compare a pixel with its
neighbor's sample. A demosaic is exactly that comparison. v4 keeps
the shuffle (into 16 channels now), then puts the stabilized mosaic
itself beside them, rebuilt at full resolution from the packed input
by the same shuffle (a `DepthToSpace`, so the ONNX contract is
unchanged), and two 3 x 3 convolutions at full resolution make RGB
from the pair (Gharbi's 2016 joint network ends the same way). It
costs about one encoder block: 0.7 s on the 24 MP frame against
0.5 s, and twelve thousand parameters.

Same data, schedule and seed as v3. CPSNR in camera space, and the
zipper score after it:

| noise | set   | hybrid       | v3           | v4           |
|-------|-------|-------------:|-------------:|-------------:|
| 0.02  | Kodak | 31.85 (10.2) | 30.10 (30.7) | 30.44 (23.0) |
| 0.02  | McM   | 30.94 (13.9) | 29.55 (30.1) | 29.89 (23.6) |
| 0.05  | Kodak | 28.94 (16.1) | 28.37 (29.7) | 28.64 (24.0) |
| 0.05  | McM   | 27.51 (18.4) | 27.65 (32.9) | 27.92 (26.7) |
| 0.1   | Kodak | 25.25 (29.9) | 26.43 (29.2) | 26.60 (27.0) |
| 0.1   | McM   | 22.83 (30.5) | 25.31 (40.5) | 25.49 (35.2) |

A quarter to a third of a decibel everywhere, the zipper down by a
quarter, and on the pepper the grid is gone to the eye at 3x where
v3's was plain; the pepper alone scores 32.26 against the hybrid's
32.19, from 31.44. What the same crop also shows is where the rest
of the gap lives: the hybrid keeps the pepper's specular glints,
single bright pixels, and v4 softens them. An L1 network at this
noise, asked about a lone bright sample, splits the difference; the
training targets, from real lenses through a low-pass filter, hold
few such samples to teach it otherwise, and the exact pairs (which
do) are a quarter of the batch. That is the next knob (`--exact-weight`
draws them more often), beside the loss (`--render-weight` adds an
L1 on the sRGB rendering two stops up, so highlight detail is paid
for as a print pays for it) and the data (66 more frames, chosen for
content this time: foliage, mountains, streets, a stained-glass
window, a cactus, sand, sky, a watch face; v5 trains on the 145).
The trainer now cuts the next batch on a thread while the GPU takes
the step, which was a fifth of the time.

On the ISO 10000 portrait v4 reads as v3 does, cleaner than the
hybrid, with the lashes a little crisper.

### v5: twice the data (2026-09-08)

v5 is v4 trained on 145 frames instead of 79: the 66 new ones were
drawn from the archive's travel, landscape and test folders (none of
the client sessions), by a seeded lottery inside each folder with a
quota, at ISO 200 or below, one from each folder in turn, and only
then looked at. CPSNR in camera space, zipper after:

| noise | set   | hybrid       | v4           | v5           |
|-------|-------|-------------:|-------------:|-------------:|
| 0.02  | Kodak | 31.85 (10.2) | 30.44 (23.0) | 30.77 (25.8) |
| 0.02  | McM   | 30.94 (13.9) | 29.89 (23.6) | 30.32 (25.5) |
| 0.05  | Kodak | 28.94 (16.1) | 28.64 (24.0) | 28.80 (24.9) |
| 0.05  | McM   | 27.51 (18.4) | 27.92 (26.7) | 28.08 (28.5) |
| 0.1   | Kodak | 25.25 (29.9) | 26.60 (27.0) | 26.74 (27.5) |
| 0.1   | McM   | 22.83 (30.5) | 25.49 (35.2) | 25.51 (36.2) |

A third to four tenths of a decibel at the low-noise end, where the
demosaic is most of the score, and little at the high end, where the
denoise is: the data was the demosaic's limit, not the denoise's, and
the gap to the hybrid at 0.02 is now 1.1 dB on Kodak and 0.6 on
McMaster. The held-out frames barely moved (47.76 against 47.69 at
σ 0.01), which says they were already covered by the first 79 and
the bench's content was not; the next frames should be chosen by
looking, since the lottery let in four of one street and four of one
cliff.

The zipper did not improve, and the pepper still carries a faint
grid at 3x, fainter than v3's, and its glints are still soft. So the
head with the mosaic beside it halved the residue and the data did
nothing to it; what is left is that nothing in the network or the
loss asks the four phases to agree on a flat field. v6 draws the
exact pairs three times as often (for the glints); a phase term in
the loss is the candidate for the grid.

### v6: the exact pairs drawn more often (2026-09-08)

v6 is v5 with `--exact-weight 3`, so the downscaled exact pairs are
about half of every batch instead of a quarter. On the bench it is a
wash: five hundredths up at 0.02 (30.81 and 30.37 in camera space),
five hundredths down at 0.1, the zipper worse at 0.1 (32.9 against
27.5 on Kodak), and the held-out native frames a decibel worse at
σ 0.1 (28.21 against 29.29). The exact pairs teach the demosaic and
not the denoise, and a real frame's noise is what the native pairs
carry; the mix stays as v5 had it. The glints are not a matter of how
often the sharp targets come round.

### v7: the phases asked to agree (2026-09-08)

The grid is the four positions of a 2x2 block disagreeing on a flat
field, so v7 adds a term that says so: the error map is split into
its four phases, each averaged over 4 x 4 of its own samples, and
the L1 distance of each from the average of all four over the same
area is added to the loss, weight 2. A smooth wrong answer costs
nothing here (the plain L1 already charges for it); a checkerboard
costs its amplitude. Otherwise v5. CPSNR in camera space, zipper
after:

| noise | set   | hybrid       | v5           | v7           |
|-------|-------|-------------:|-------------:|-------------:|
| 0.02  | Kodak | 31.85 (10.2) | 30.77 (25.8) | 31.07 (22.4) |
| 0.02  | McM   | 30.94 (13.9) | 30.32 (25.5) | 30.38 (23.0) |
| 0.05  | Kodak | 28.94 (16.1) | 28.80 (24.9) | 28.98 (21.9) |
| 0.05  | McM   | 27.51 (18.4) | 28.08 (28.5) | 28.15 (25.2) |
| 0.1   | Kodak | 25.25 (29.9) | 26.74 (27.5) | 26.81 (22.6) |
| 0.1   | McM   | 22.83 (30.5) | 25.51 (36.2) | 25.61 (31.2) |

Better in every cell, the zipper down by a tenth to a fifth, the
held-out frames unmoved (47.73 at σ 0.01), and on the pepper at 3x
the grid is gone: the term did what it was for and nothing else. The
pepper alone is 32.56 against the hybrid's 32.19. At 0.05 the
network is now ahead of the hybrid on both sets, and at 0.02 the gap
is 0.8 dB on Kodak and 0.6 on McMaster, all of it demosaic: the
glints are still soft where the hybrid keeps them, and the zipper
is still twice the hybrid's, which by now is not a grid but edges.
Best model so far; `runs/v7/denoise-v7.onnx`.

The day's order of business, then, for the runs that follow: the
rendered-domain term (`--render-weight`, ready and untried) for the
highlights; then width in the first level or in the head for the
edges; then frames chosen by content; and the plumbing waits on
0.02, where the hybrid is still the better demosaic.

### The near-clean cell (2026-09-08, evening)

What the bench had not yet said is how v7 does where a real file at
base ISO sits, so a cell at noise 0.005 (the R6 II at ISO 800 is
0.006). CPSNR in camera space, then the per-channel PSNR R, G, B:

| set   | v7                       | hybrid                   | AMaZE alone              |
|-------|-------------------------:|-------------------------:|-------------------------:|
| Kodak | 31.82 (31.1, 34.0, 31.1) | 33.52 (33.6, 34.6, 32.8) | 34.00 (34.6, 36.2, 32.3) |
| McM   | 31.38 (31.6, 33.2, 30.2) | 32.69 (33.7, 34.7, 30.9) | 32.27 (34.0, 35.2, 29.9) |

So 1.3 to 1.7 dB behind at base ISO, which is the number the plan's
ISO gate would be set by if nothing closes it. Where it is lost is
plain in the channels: green is within half a decibel of AMaZE, red
and blue are two and a half behind. The network demosaics luminance
about as well as the best hand-written method and chroma worse than
it, and chroma is what the head, with sixteen channels and one
sample per pixel at full resolution, has the least room for: the red
and blue samples are one in four, and the interpolation between them
needs the context the half-resolution decoder has and the head
cannot carry across in sixteen channels. A wider head is the first
thing to try for it, after v8 (the rendered-domain term) reports.

### v8: the rendered-domain term (2026-09-09)

v8 is v7 with `--render-weight 4`: an L1 on the sRGB rendering two
stops up, beside the stabilized L1 and the phase term. It is a tenth
of a decibel behind v7 in every cell, in camera space: 31.69 and
31.32 at 0.005, 30.95 and 30.33 at 0.02, 28.87 and 28.12 at 0.05,
26.74 and 25.52 at 0.1, the zipper level with v7's. The glints did
not come back either. So the stabilized L1 was already weighing the
highlights as well as they can be weighed, and the term only added
a second opinion that pulled a little the wrong way; it is off from
here. The softness is not a matter of where the error is paid for.

### v9: a wider head, and what WebGPU did with it (2026-09-09)

v9 is v7 with the head's full-resolution channels doubled, 16 to 32,
for the red and blue. Its first bench came back at 8 dB, garbage,
while the same file verified against PyTorch to 1e-5 on the CPU
provider. A `--provider cpu|webgpu` flag on the bench and a set of
random-weight variants found the trigger: ONNX Runtime's WebGPU
convolution answers wrongly, and silently, for the 3 x 3 conv with 33
input channels (32 features and the mosaic), where 17, 25 and 36 are
right. The trainer now pads that concat with zero planes to a
multiple of four channels, with zero weights that stay zero, so the
graph is the same function; a checkpoint from before the padding
loads through the same path. Two lessons for the record: a provider
can be wrong without being loud, so a new graph is checked CPU
against WebGPU before its numbers are believed; and the runtime
side's "first provider that runs the model" cannot catch this, since
the model runs.

With that, CPSNR in camera space, zipper after, v9 on either
provider:

| noise | set   | hybrid       | v7           | v9           |
|-------|-------|-------------:|-------------:|-------------:|
| 0.005 | Kodak | 33.52 ( 9.2) | 31.82 (20.8) | 33.03 (15.9) |
| 0.005 | McM   | 32.69 (12.7) | 31.38 (20.9) | 32.20 (16.9) |
| 0.02  | Kodak | 31.85 (10.2) | 31.07 (22.4) | 31.96 (17.7) |
| 0.02  | McM   | 30.94 (13.9) | 30.38 (23.0) | 30.96 (19.2) |
| 0.05  | Kodak | 28.94 (16.1) | 28.98 (21.9) | 29.41 (18.2) |
| 0.05  | McM   | 27.51 (18.4) | 28.15 (25.2) | 28.49 (21.6) |
| 0.1   | Kodak | 25.25 (29.9) | 26.81 (22.6) | 27.04 (18.6) |
| 0.1   | McM   | 22.83 (30.5) | 25.61 (31.2) | 25.74 (26.7) |

Nine tenths of a decibel at the low end and a quarter at the high,
the zipper down by a fifth again, and the channels say it went where
it was aimed: red and blue on Kodak at 0.005 went from 31.1 to 32.4
and 32.2 while green moved 34.0 to 35.3. The network is now level
with the hybrid at 0.02 and ahead above it, and half a decibel
behind at 0.005, from 1.7. On the pepper the glints are back. The
head was the bottleneck, and 32 channels is unlikely to be the end
of it: v11 tries 64. Inference on the 24 MP frame is 0.8 s from
0.6.

The held-out frames moved less than the bench (47.80 at σ 0.01
from 47.73), as they have all along; they are AMaZE renders of
lens-blurred frames, and a network that has learned to demosaic
better than AMaZE cannot show it against them. The bench, whose
truth is the image, is the number to steer by from here.

### v10: a wider first level (2026-09-09)

v10 is v7 with the first encoder level at 48 channels instead of 32,
head still at 16, to set the two widths against each other. CPSNR
in camera space, zipper after:

| noise | set   | v7 (32, 16)  | v10 (48, 16) | v9 (32, 32)  |
|-------|-------|-------------:|-------------:|-------------:|
| 0.005 | Kodak | 31.82 (20.8) | 32.62 (20.8) | 33.03 (15.9) |
| 0.005 | McM   | 31.38 (20.9) | 32.23 (20.2) | 32.20 (16.9) |
| 0.02  | Kodak | 31.07 (22.4) | 31.57 (22.7) | 31.96 (17.7) |
| 0.02  | McM   | 30.38 (23.0) | 30.80 (23.2) | 30.96 (19.2) |
| 0.05  | Kodak | 28.98 (21.9) | 29.27 (21.6) | 29.41 (18.2) |
| 0.1   | Kodak | 26.81 (22.6) | 27.04 (21.7) | 27.04 (18.6) |

Both widths pay, and the head pays more for less: v10 costs 1.1 s on
the 24 MP frame against v9's 0.8, gains half to eight tenths at the
low end against v9's nine tenths to 1.2, and leaves the zipper where
v7 had it while v9 cut it by a fifth. The first level works at half
resolution and sees the packed channels; the head works at full
resolution and sees the samples; the demosaic's fine decisions are
made in the second. So the head goes first (v11, 64 channels), and
the first level after, on top of whichever head wins. A note for the
method: v10 spent its first forty minutes starved while the CPU
provider benches ran on every core, so a CPU bench and a training
run do not share the machine; the WebGPU benches do.

### v11: the head at 64, and the hybrid passed in every cell (2026-09-09)

v11 is v9 with the head at 64 channels. CPSNR in camera space,
zipper after:

| noise | set   | hybrid       | v9           | v11          |
|-------|-------|-------------:|-------------:|-------------:|
| 0.005 | Kodak | 33.52 ( 9.2) | 33.03 (15.9) | 33.63 (14.5) |
| 0.005 | McM   | 32.69 (12.7) | 32.20 (16.9) | 32.74 (15.7) |
| 0.02  | Kodak | 31.85 (10.2) | 31.96 (17.7) | 32.28 (16.4) |
| 0.02  | McM   | 30.94 (13.9) | 30.96 (19.2) | 31.20 (18.3) |
| 0.05  | Kodak | 28.94 (16.1) | 29.41 (18.2) | 29.62 (17.0) |
| 0.05  | McM   | 27.51 (18.4) | 28.49 (21.6) | 28.62 (20.8) |
| 0.1   | Kodak | 25.25 (29.9) | 27.04 (18.6) | 27.20 (18.6) |
| 0.1   | McM   | 22.83 (30.5) | 25.74 (26.7) | 25.79 (25.5) |

Another half a decibel at the low end, and with it the network is
ahead of the hybrid in every cell of the bench, base ISO included,
by a tenth there and by two to three decibels at the top. Green on
Kodak at 0.005 is 36.0 against AMaZE's 36.2 and red and blue are
32.9 against its 34.6 and 32.3, so what the hybrid still does better
is the red channel at base ISO and the zipper, half again the
hybrid's on edges. The cost is 1.6 s on the 24 MP frame, from 0.8:
the head at full resolution is now most of the network's work, and
each doubling has bought about half a decibel for twice the time.
The roadmap's condition for the plumbing (a model that beats the
hybrid on the bench) is met by this one, `runs/v11/denoise-v11.onnx`.
v12 puts v10's wider first level under this head, to see whether the
two add.

### v12: both widths (2026-09-09)

v12 is v11 with the first encoder level at 48, v10's change under
v11's head. CPSNR in camera space, zipper after, with AMaZE alone
(no denoise) in the near-clean row since that is now the comparison:

| noise | set   | hybrid       | AMaZE alone  | v11          | v12          |
|-------|-------|-------------:|-------------:|-------------:|-------------:|
| 0.005 | Kodak | 33.52 ( 9.2) | 34.00 (22.8) | 33.63 (14.5) | 34.48 (13.6) |
| 0.005 | McM   | 32.69 (12.7) | 32.27 (23.9) | 32.74 (15.7) | 33.41 (14.7) |
| 0.02  | Kodak | 31.85 (10.2) |              | 32.28 (16.4) | 32.84 (15.4) |
| 0.02  | McM   | 30.94 (13.9) |              | 31.20 (18.3) | 31.64 (17.4) |
| 0.05  | Kodak | 28.94 (16.1) |              | 29.62 (17.0) | 29.94 (15.5) |
| 0.05  | McM   | 27.51 (18.4) |              | 28.62 (20.8) | 28.93 (19.6) |
| 0.1   | Kodak | 25.25 (29.9) |              | 27.20 (18.6) | 27.41 (15.5) |
| 0.1   | McM   | 22.83 (30.5) |              | 25.79 (25.5) | 26.06 (24.6) |

The two widths add: half to nine tenths of a decibel over v11, and
at base ISO the network is now past AMaZE with no denoiser at all
(34.48 against 34.00 on Kodak, 33.41 against 32.27 on McMaster),
which was the ceiling the first day's notes thought the head could
not reach. The first level costs little at inference (1.7 s on the
24 MP frame against v11's 1.6), since it works at half resolution.
The zipper is at 13.6 against the hybrid's 9.2, and the coarse error
is the lowest of any method at every noise. v13 takes the first
level to 64; after that a longer schedule on the winner, since every
run still improves at the cosine tail.

### v13: the first level at 64 (2026-09-09)

v13 is v12 with the first level at 64. CPSNR in camera space,
zipper after:

| noise | set   | v12          | v13          |
|-------|-------|-------------:|-------------:|
| 0.005 | Kodak | 34.48 (13.6) | 34.86 (12.6) |
| 0.005 | McM   | 33.41 (14.7) | 33.70 (14.1) |
| 0.02  | Kodak | 32.84 (15.4) | 33.08 (14.5) |
| 0.02  | McM   | 31.64 (17.4) | 31.87 (16.6) |
| 0.05  | Kodak | 29.94 (15.5) | 30.11 (14.9) |
| 0.05  | McM   | 28.93 (19.6) | 29.10 (18.8) |
| 0.1   | Kodak | 27.41 (15.5) | 27.52 (15.1) |
| 0.1   | McM   | 26.06 (24.6) | 26.06 (24.4) |

Two to four tenths at the low end, nothing at the top, the zipper a
little lower, 1.8 s on the 24 MP frame. Smaller than the step before
it, as the second doubling of anything is; the shape is now 64 at the
first level, 64 in the head, 2.26 M parameters, and it is the one to
train longer. v14 is v13 at 120 000 steps, since every run has still
been improving as the learning rate reached its floor.

### v14: twice the steps (2026-09-09)

v14 is v13's shape at 120 000 steps, the cosine stretched to match.
CPSNR in camera space, zipper after:

| noise | set   | hybrid       | v13          | v14          |
|-------|-------|-------------:|-------------:|-------------:|
| 0.005 | Kodak | 33.52 ( 9.2) | 34.86 (12.6) | 35.40 (11.9) |
| 0.005 | McM   | 32.69 (12.7) | 33.70 (14.1) | 34.17 (13.4) |
| 0.02  | Kodak | 31.85 (10.2) | 33.08 (14.5) | 33.40 (13.7) |
| 0.02  | McM   | 30.94 (13.9) | 31.87 (16.6) | 32.12 (15.8) |
| 0.05  | Kodak | 28.94 (16.1) | 30.11 (14.9) | 30.29 (13.9) |
| 0.05  | McM   | 27.51 (18.4) | 29.10 (18.8) | 29.30 (17.9) |
| 0.1   | Kodak | 25.25 (29.9) | 27.52 (15.1) | 27.72 (14.1) |
| 0.1   | McM   | 22.83 (30.5) | 26.06 (24.4) | 26.32 (22.8) |

Two to five tenths in every cell, the zipper lower again, and no
sign of the run fitting the archive: the held-out frames rose with
the bench (48.08 from 48.02 at σ 0.01, 30.28 from 29.88 at 0.1),
which is the pattern of a run that was short, not one that was
learning its own data. The network is now 1.2 to 1.9 dB ahead of the
hybrid at base ISO and 2.5 to 3.5 ahead at 0.1, at 1.8 s on the
24 MP frame. v15 doubles the steps again, to 240 000, for the night;
the second doubling of a schedule usually pays half the first.

### Other people's cameras (2026-09-09, evening)

682 raws arrived from two other photographers: mostly Canon test
shots from lens and body comparisons (R5, R5 II, R6, R6 II, R6 III,
R7, R8), and a Nikon Z6 III, a Sony A7 IV and a Panasonic S5 II,
each with a few frames at ISO 25 600 beside its base-ISO ones. The
survey read every header; the Sony and Panasonic files decode and
come out RGGB with sensible gains; 48 of the 62 Nikon files are the
Z6 III's high-efficiency compression, which rawler does not support,
so seven Nikon frames came through and the rest wait on the
decoder. 65 frames were exported for training (every foreign
low-ISO frame, and 50 Canon at ISO 200 or below drawn one per
folder in turn), and every frame above ISO 3200 was held out
untouched, 36 raws, since a sensor the network has never seen at a
noise beyond anything in its training is the test of the whole
design: the stabilizer is supposed to make sensors interchangeable.

It does. The Sony at ISO 25 600 measures σ 0.029 at mid grey, past
the widest noise the training draws (0.06 is the top of the range,
but the archive's own frames reach 0.016) and on a sensor, a
color matrix and a lens the network has never met; v14 renders the
face clean with the eyes sharp and the lips drawn, where the hybrid
leaves color blotches across the skin, and the tweed of a jacket
keeps its weave where the hybrid smears it. The Panasonic at ISO
25 600 (σ 0.020) is the same story: skin texture and hair strands
kept, the hybrid waxy. No number yet, since there is no truth for a
real frame; the eye is the judge here, and it is not close.

For the larger set the loader now memory-maps the native frames
instead of copying them into RAM (the downscaled pairs are still
built at start and held), so the set can be larger than memory and
the page cache holds what fits; a crop reads its own rows and the
pair is bit-identical to before. v16 is v14's run on the 210 frames,
queued behind v15.

A blind test the same evening: nine 700 px crops from the held-out
raws (three Sony, three Panasonic, three Canon R5 II, ISO 6400 to
25 600), hybrid and v14 side by side in a seeded random order, judged
by the photographer without the key. v14 was preferred in eight of
nine; the one the hybrid took (a Sony frame at ISO 12 800) was
called a slight edge, and the two color artifacts the judge saw were
both the hybrid's. Every Canon pair was called "so close", which
matches the bench: on the sensor the network trained on, at the
noise the archive reaches, the two are near level, and the margin
opens on sensors and noise it never saw.

More Nikon Z6 III raws arrived the same night, 25 at ISO 100 to
8000; 16 are the high-efficiency compression again (17 to 19 MB
files, where the lossless ones run 26 to 32 MB) and do not decode.
Five at ISO 1250 to 8000 joined the held-out set, 41 raws, and the
ISO 100 frame joined the training export. A second blind test on
those five, full frames this time: three were called a wash (ISO
1600 to 3200; one side had redder flowers, which was the network's),
and the judge picked the network's frame correctly on the other two
(ISO 1250 and 8000). That is the shape the bench predicts: at the
noise this sensor makes below ISO 3200 the margin is measurable but
not visible at fit-to-screen, and the eye can only tell them apart
once the noise is past what the hybrid handles.

### v15: the schedule doubled again (2026-09-10)

v15 is v14's run at 240 000 steps, 9 hours in two sittings with a
reboot between (the checkpoint resumed at 144 000 without a seam in
the curve). Camera-space CPSNR, v15 against v14: 0.005 Kodak 35.73
from 35.40, McM 34.10 from 34.17; 0.02 33.61 from 33.40, 32.13 from
32.12; 0.05 30.45 from 30.29, 29.39 from 29.30; 0.1 27.82 from 27.72,
26.34 from 26.32. The held-out frames moved the same way, 30.7 from
30.3 at σ 0.1 and level elsewhere. So the second doubling bought a
tenth to a third of a dB on Kodak and nothing on McM, a third of what
the first doubling did, as the rule of thumb says; the schedule is
near the point where more steps buy less than more data or more
width would. v15 is the model to ship unless v16, the same run on the
210 frames with the foreign sensors in, moves the foreign held-out
frames; the bench cannot see that, since it is one Canon's color.

A note on measuring beside a run: bench timings taken while the GPU
is training are meaningless (v15 read 18 to 24 s/MP against 1.8 s
alone), and the tool that runs the evaluation was killed once for
memory while v16 loaded its frames, so the cells were re-run one at a
time. The numbers themselves do not depend on contention.

### v16: the foreign sensors in the training set (2026-09-10)

v16 is v14's recipe on all 210 frames, the 65 foreign ones included,
6.6 hours on the memory-mapped loader (the frames are read as they
are cut rather than held, and the 210-frame set built its downscaled
pairs for the first 20 minutes). The bench cannot tell it from v14:
0.005 35.55 / 34.10, 0.02 33.42 / 32.09, 0.05 30.32 / 29.32, 0.1
27.73 / 26.32, every cell within 0.15 dB of v14's and below v15's.
The point of v16 was the sensors the bench cannot see, so v15 and
v16 rendered the 34 decodable held-out foreign raws side by side
(the seven older Nikon files in that folder are the high-efficiency
compression and were never decodable; the folder was filled by ISO
without a decode check). On the Sony and Panasonic at ISO 25 600 the
two are the same picture: the weave of the jacket, the button, the
faint color speckle in the black behind it, all present in both,
and the whole frame differs by 0.3 percent of range on average,
under one code value in eight bits. So the foreign frames at low ISO
taught the network nothing it did not already know from the Canon
archive, which is the stabilizer doing its job, and the extra data
is not a lever either. What is left is width, and the trade against
speed. A full 24 MP develop with either model is 3.0 s wall, decode
and the PNG write included, against 5.1 s for the hybrid.

### v17: wider everywhere, and a GPU that fails without a word (2026-09-10)

v17 is v14's recipe with every width half again: 96, 96, 192, 384
and a 96-channel head, 5.3 M parameters, 120 000 steps in 10 hours
at 13.3 GB of VRAM. Camera-space CPSNR against v15, which had twice
the steps: 0.005 Kodak 36.17 from 35.73, McM 34.38 from 34.10; 0.02
33.82 from 33.61, 32.34 from 32.13; 0.05 30.58 from 30.45, 29.59 from
29.39; 0.1 27.92 from 27.82, 26.55 from 26.34. So width still pays
where the schedule and the data have stopped, on McM as well as
Kodak, at 120k steps against v15's 240k; the same shape at 240k is
the next run once the queue clears. Inference is about twice v15's.

Its first bench read 7 dB on WebGPU and 30 on the CPU, which looked
like the 33-channel bug again, so it got the same treatment:
random-weight variants of every shape between v15's and v17's, CPU
against WebGPU. Every one wider than v15 failed and no pattern in
the channels explained it, and a probe that runs a single graph on
both providers (`crates/greycard-ai/examples/probe.rs`) found every
one of them correct, every single convolution correct, and every
shape correct at 736 x 736 packed. The difference was the tile: the
Denoiser cuts 1536-pixel tiles with a 96-pixel margin, so the
network always runs at 864 x 864 packed whatever the image, and at
that size v15 takes 6.4 GB of VRAM and the wider shapes need more
than the 8 GB that v18's training had left free. The WebGPU provider
does not report that. It returns garbage, and on the way out
corrupts the heap (`free(): invalid pointer`), and the same happens
to v15 itself at 1120 x 1120. With 1024-pixel tiles v17 on WebGPU
matches the CPU to the last digit in every cell. Three lessons: a
bench beside a training run is not just slow, it can be wrong; a
provider mismatch is a memory question before it is a maths
question; and the Denoiser must not assume 6.4 GB is free, which on
an 8 GB card beside a browser it is not. The tile wants choosing
from the memory the device has, or at least a much smaller default
and a loud failure; that is the runtime side's, on the roadmap. The
bench took `--tile` so an evaluation can be run beside training.

### v18: the 512 patch (2026-09-11)

v18 is v14's model trained on 512-pixel patches at batch 16, the
same pixels per step, 5.8 hours. It is a little worse than v14 in
every cell: 0.005 Kodak 35.30 from 35.40, McM 34.12 from 34.17; 0.02
33.33 from 33.40, 32.08 from 32.12; 0.05 30.23 from 30.29, 29.23 from
29.30; 0.1 27.64 from 27.72, 26.12 from 26.32, and the held-out
frames the same, 29.2 from 30.3 at σ 0.1. So the network's reach is
not short of what it uses: giving it more context per patch bought
nothing, and sixteen patches per step instead of sixty-four cost a
little in the noise of the gradient, most at the noisiest cell. That
settles the depth question for now. A fifth level would buy reach
the network is not asking for, and width is the lever, which v20 is
pulling.

### v19: the small model (2026-09-11)

v19 is the speed tier: widths 48, 48, 96, 192 and a 32-channel
head, 1.2 M parameters, v14's recipe otherwise, 240 000 steps on all
210 frames. It took 14 hours, longer than the bigger v14, because
the loader is one thread cutting a batch in about 200 ms and this
network's step is well under that; the GPU sat a quarter busy. The
loader wants several workers, which is the next change once v20 has
finished and the comparison is not disturbed. Camera-space CPSNR:
0.005 Kodak 35.18, McM 33.88; 0.02 33.23, 31.91; 0.05 30.17, 29.15;
0.1 27.59, 26.08. That is 0.2 to 0.3 dB under v14 and 0.5 under v15
at base ISO, and still 1.2 to 1.7 dB past the hybrid there and 2.3
to 3.2 at 0.1, from a network with about half the parameters
and, by the count of multiplies, about 45 percent of the time. Its
speed on a frame is measured once the GPU is free.

### v20: the wide model at 240k (2026-09-13)

v20 is v17's shape, widths 96, 96, 192, 384 and a 96-channel head,
at 240 000 steps on all 210 frames, 20 hours. Camera-space CPSNR
against v17 (same shape, 120k): 0.005 Kodak 36.41 from 36.17, McM
34.35 from 34.38; 0.02 33.98 from 33.82, 32.38 from 32.34; 0.05
30.67 from 30.58, 29.68 from 29.59; 0.1 27.82 from 27.92, 26.44 from
26.55. The held-out frames moved more: 31.9 from 31.1 at σ 0.1, and
the zipper column fell in every cell (10.6 against v17's 11.2 at
0.005; the hybrid's 9.2 is now close). So the second doubling bought
the wide model what it bought the narrow one, a tenth or two on
Kodak, and at 0.1 the bench and the held-out frames disagree: the
bench says a tenth worse, the frames say eight tenths better. The
bench's 0.1 cell is Kodak and McM content the network never saw,
scored at a noise past the archive's; the held-out frames are real
raws. Both count, and the ranking between v17 and v20 at 0.1 is
within the bench's run-to-run spread, so v20 stands as the quality
tier.

Wall time for a full 24 MP develop on WebGPU, decode and the PNG
write included, GPU otherwise idle: v19 2.4 s, v15 3.3 s, v20 4.2 s,
and AMaZE with no denoise 1.5 s. So the three tiers are roughly one,
two and three seconds of network on this card, for 35.2, 35.7 and
36.4 dB at base ISO on Kodak against the hybrid's 33.5.

The queue is done. Three runs left to make: the loader with several
workers (v19 was starved), and after that whatever the loader's
speed makes affordable. What has stopped paying: steps past 240k,
data past the archive, the patch, and any loss term tried so far.

### The loader was the disk (2026-09-13)

v19's slow run was blamed on one thread cutting batches. Measured
with the page cache warm, one thread cuts a batch in 15 ms; the
200 ms seen in training was the cache cold. The downscaled pairs,
16 GB of float16 tensors, lived in the process, which left the cache
too small for the 46 GB of native frames, so a random crop was a few
hundred reads from the NVMe and every batch paid for them. Now a
downscaled pair is built once and written beside its frame as
``STEM.s2.mosaic.npy`` and the like, memory-mapped like the native
pair and bit-identical to the tensors of before; the process holds
no frames, and the cache holds most of the 62 GB. The trainer cuts
with several threads (``--workers``, six by default), each batch from
a generator seeded by the run's seed and the batch's index, so the
random stream is the same whatever the worker count and wherever a
run resumed. The v19 shape now trains at 90 ms a step against 210,
and a 240k run of it is six hours rather than fourteen; the big
models are GPU-bound and gain less. The first run on a data set
builds the cache, about ten minutes for 210 frames.
