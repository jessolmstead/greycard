# 13. Image quality first, and the GPL (2026-09-05)

**Decision: image quality is the first goal of greycard, above library
reach.** The engine is relicensed GPL-3.0-or-later (was MIT OR Apache-2.0;
`LICENSE-MIT` and `LICENSE-APACHE` removed, `LICENSE` is GPL-3 at the root
and in each crate). Reasoning:

- The best open Bayer pipeline is RawTherapee's (AMaZE, dual demosaic, CA
  correction before demosaic, highlight reconstruction) and it is GPL with no
  independent specification. A permissive engine would have to clean-room
  it from a description that does not exist. GPL lets us port with
  attribution.
- rawler is LGPL and links statically, so a permissive engine crate was
  already less useful to permissive consumers than it looked (§8).
- `rawcolor` is untouched: permissive, and separately published.

**Porting rules.** A ported file's header names the source project, file,
authors and license. Port the algorithm, not the surrounding plumbing;
greycard's types and threading stay ours. Every port gets the same
treatment as a native op: CPU reference, tests, and a benchmark score.

**Plan, in order.**

1. **Demosaic benchmark harness** before any demosaic. Reference images
   (Kodak and McMaster sets), mosaicked with a Bayer pattern, demosaicked,
   scored: PSNR per channel and CPSNR, plus a zipper and a false-color
   measure. Published numbers exist for bilinear, RCD and AMaZE, so a port
   or a from-scratch implementation is checked against the literature. Add
   200% crops of the real test files next to RawTherapee's output for what
   PSNR misses (maze artifacts, color moiré). Every demosaic change has to
   move the numbers.
2. **RCD** from Rodríguez's published description: compact, fast, the
   natural GPU reference, darktable's default.
3. **AMaZE** ported from RawTherapee, then **dual demosaic** (AMaZE or RCD
   with VNG4 in flat, noisy regions) for high ISO.
4. After demosaic, the things that visibly change a print, in order:
   **highlight reconstruction** (clip-to-neutral is the floor; the blown
   areas on the orchids show it), **chromatic aberration correction before
   demosaic** (removes the fringing every demosaic then has to hide),
   **profiled noise reduction** as a first-class feature.
5. GPU implementations follow each CPU reference, tested against it.

### 13a. Benchmark harness built (2026-09-05)

`crates/greycard-bench` (`greycard-bench DIR...`), data fetched by
`scripts/fetch-bench-data.sh` into gitignored `data/bench/{kodak,mcm}`
(the McMaster zip is encrypted; the password is on the dataset page and in
the script). Scores 8-bit sRGB over an interior that drops 8 px per side:
PSNR per channel, CPSNR, Lu–Tan zipper percentage (threshold 2.5 ΔE),
mean CIE76 ΔE and its chroma part. `--linear` linearizes before mosaic and
re-encodes after, which is what the engine actually sees; the default is
the literature's gamma-domain convention. `--dump DIR` writes outputs and
8x amplified error images. `DemosaicMethod` in core is the enumeration it
runs over; `develop --demosaic NAME` selects one in the CLI.

Bilinear baseline, RGGB, means:

| set    | domain | PSNR R | PSNR G | PSNR B | CPSNR | zipper % | ΔE    | Δab   |
|--------|--------|-------:|-------:|-------:|------:|---------:|------:|------:|
| Kodak  | sRGB   | 29.32  | 33.14  | 29.33  | 30.26 | 38.19    | 4.172 | 3.889 |
| Kodak  | linear | 29.05  | 32.61  | 28.91  | 29.88 | 38.37    | 4.304 | 4.003 |
| McM    | sRGB   | 31.67  | 35.39  | 31.22  | 32.31 | 27.23    | 3.493 | 3.275 |
| McM    | linear | 31.02  | 34.57  | 30.55  | 31.62 | 27.75    | 3.706 | 3.471 |

The Kodak figures sit where published bilinear results sit (high 20s red
and blue, low 30s green), which is the check that the harness itself is
right. Exact literature comparisons depend on border and pattern
conventions; when RCD and AMaZE land, compare against papers that state
theirs. Bilinear costs about 25 ms per megapixel on this machine, single
image, all cores.

### 13b. RCD ported (2026-09-05)

`develop::rcd`, ported from Luis Sanz Rodríguez's reference release 2.3
(GPL-3, attribution in the file header). Row-parallel; bilinear seeds every
buffer so the 4 px border is bilinear and the ring inside it never reads
zeros as the reference does. Output is clamped to `0..=max(1, largest
sample)` rather than `0..=1` so the unclipped DNG path keeps its highlights.
Now the default in `DevelopSettings` and the CLI.

Benchmark, RGGB, means (bilinear from §13a for comparison):

| set   | domain | method   | PSNR R | PSNR G | PSNR B | CPSNR | zipper % | ΔE    |
|-------|--------|----------|-------:|-------:|-------:|------:|---------:|------:|
| Kodak | sRGB   | bilinear | 29.32  | 33.14  | 29.33  | 30.26 | 38.19    | 4.172 |
| Kodak | sRGB   | **rcd**  | 37.14  | 37.19  | 36.73  | 37.00 | 12.51    | 2.058 |
| McM   | sRGB   | bilinear | 31.67  | 35.39  | 31.22  | 32.31 | 27.23    | 3.493 |
| McM   | sRGB   | **rcd**  | 36.18  | 39.29  | 34.50  | 36.12 | 12.31    | 2.482 |
| Kodak | linear | rcd      | 35.70  | 36.85  | 33.48  | 34.99 | 16.46    | 2.538 |
| McM   | linear | rcd      | 34.90  | 38.86  | 30.80  | 33.55 | 19.10    | 3.133 |

About 6.7 dB over bilinear on Kodak, zipper rate a third of bilinear's.
The linear-domain rows score lower for every method because re-encoding
to sRGB stretches shadow errors; blue suffers most. Worth remembering when
comparing against papers, which all use the gamma domain. RCD costs about
6x bilinear: ~200 ms/MP here with images running in parallel, so the
single-image figure is better than that. No verified literature figure for
RCD is quoted here; find one that states its border and pattern before
comparing.

### 13c. AMaZE ported (2026-09-05)

`develop::amaze`, ported from RawTherapee's `amaze_demosaic_RT.cc` (Emil
Martinec, optimized by Ingo Weyrich; GPL-3, attribution in the file
header), scalar branch. Same 160 px tiles with 16 px reflected margins as
the original, tiles run in parallel with per-thread scratch, output
scattered afterwards. Departures, all in the margins: consistent corner
reflection (the original mirrors corners about a different row), zeroed
buffers instead of aliased ones, bilinear for the outer 3 px instead of the
original's border routine. Clip point is 1.0 because samples are
green-normalized (the original's `1/initialGain`). Now the default.

Benchmark, RGGB, means:

| set   | domain | method   | PSNR R | PSNR G | PSNR B | CPSNR | zipper % | ΔE    |
|-------|--------|----------|-------:|-------:|-------:|------:|---------:|------:|
| Kodak | sRGB   | rcd      | 37.14  | 37.19  | 36.73  | 37.00 | 12.51    | 2.058 |
| Kodak | sRGB   | **amaze**| 39.06  | 40.71  | 38.28  | 39.22 | 6.80     | 1.690 |
| McM   | sRGB   | rcd      | 36.18  | 39.29  | 34.50  | 36.12 | 12.31    | 2.482 |
| McM   | sRGB   | amaze    | 35.82  | 39.43  | 34.29  | 35.96 | 11.48    | 2.545 |
| Kodak | linear | rcd      | 35.70  | 36.85  | 33.48  | 34.99 | 16.46    | 2.538 |
| Kodak | linear | amaze    | 36.64  | 39.86  | 33.20  | 35.58 | 12.70    | 2.353 |
| McM   | linear | rcd      | 34.90  | 38.86  | 30.80  | 33.55 | 19.10    | 3.133 |
| McM   | linear | amaze    | 34.50  | 39.06  | 29.87  | 32.88 | 19.83    | 3.372 |

AMaZE is 2.2 dB ahead on Kodak with half the zipper rate, and 0.16 dB
behind RCD on McMaster, which has saturated color edges where AMaZE's
diagonal chroma interpolation gains nothing. That split is the two
algorithms' known character (RawTherapee defaults to AMaZE, darktable to
RCD), so it reads as a faithful port. AMaZE is the default because Kodak
is closer to photographs; RCD stays as the fast option and the candidate
for the smooth half of a dual demosaic. Cost: the bench's ms/MP column
says twice RCD, but it runs images in parallel and RCD's row-parallel
passes share cores worse than AMaZE's tiles; on the 24 MP orchids the
whole `develop` run takes the same time with either (about 0.9 s).

### 13d. Highlight reconstruction: inpaint opposed (2026-09-05)

`develop::highlights`, ported from darktable's `hlreconstruct/opposed.c`
(Hanno Schwalm with garagecoder and Iain of G'MIC; GPL-3, attribution in
the file header). Runs on the white-balanced mosaic before the demosaic,
where channel `c` saturates at its gain: a clipped photosite becomes the
opposed average (mean of the other two channels' 3x3 means, in cube-root
space) plus a per-channel chrominance offset measured on unclipped
photosites within about three 3x3 blocks of clipped ones. Clip level is
gain × 0.987, the reference's magic.

**Departure:** where every photosite in the 3x3 neighborhood is clipped,
the reference's output leans toward the highest-gain channel and relies on
the tone mapper to whiten it. This engine has no tone mapper, so such
photosites go to neutral at the brightest channel's clip level. With
reconstruction on, `develop` clips at max(gains) instead of 1.0 and the
working image legitimately exceeds 1.0 in highlights; the consumer's tone
mapping rolls that off. `HighlightMode::Clip` keeps the old behavior.
The linear DNG path is untouched: the consumer reconstructs.

**Measured.** The bench gained `--overexpose STOPS --highlights MODE`:
the reference is boosted, each channel clipped at a daylight Canon's gains
(1.9, 1.0, 1.8), handled, demosaicked, brought back down and scored against
the unclipped original. Linear domain, AMaZE:

| set   | EV | clip CPSNR | opposed CPSNR | gain    |
|-------|---:|-----------:|--------------:|--------:|
| Kodak | +1 | 28.10      | 33.38         | +5.3 dB |
| McM   | +1 | 27.87      | 32.31         | +4.4 dB |
| Kodak | +2 | 20.89      | 26.28         | +5.4 dB |

On the orchids (163 k clipped photosites, chrominance R +0.10 G +0.01
B +0.34 from 11 k / 53 k / 13 k votes) the blown background at −1.5 EV
comes back bright and white where the clip version is flat grey; the
bluish edge where only some channels clip is scene color and appears in
both. Zipper and ΔE columns in the overexposure runs are dominated by the
clipped regions and are not comparable to the demosaic tables.

**Next for highlights:** darktable's segmentation-based mode for large
clipped areas with structure, and a proper clipped-region-only metric in
the bench.

### 13e. Lateral chromatic aberration correction (2026-09-05)

`develop::ca`, ported from RawTherapee's `CA_correct_RT.cc` (Emil
Martinec; iterated correction and color-shift avoidance by Ingo Weyrich;
GPL-3, attribution in the file header), automatic mode only. Runs on the
white-balanced mosaic after highlight reconstruction and before the
demosaic, in the same order darktable uses. Per 128 px tile it
interpolates green at the red and blue sites, solves in closed form for the
sub-pixel offset that minimizes color-difference variance along each
axis, medians and variance-gates the tile votes, fits a fourth-order
polynomial in tile position (second-order with under 32 voting tiles),
resamples red and blue from where the polynomial says they landed with the
reference's overshoot guards, iterates twice, then blurs the old-to-new
ratio widely and applies it so no large area changes color. On by
default; `--no-ca` in the CLI.

**Departures.** Consistent corner reflection and zeroed buffers as usual.
A triple box blur stands in for the reference's recursive Gaussian in the
color-shift step. And one fix: the reference's reduced-order fallback
reads the wrong entries of its full normal matrix (it treats the first
sixteen entries of the 16x16 as a 4x4), so with fewer than 32 voting tiles
it fits garbage; the port builds the four-parameter system from the right
basis entries. That fallback never runs on a camera-sized frame (a 24 MP
image has about 1900 tiles), which is presumably why it survived.

**Measured.** The bench gained `--aberrate PIXELS [--no-ca]`: red magnified
about the center so it lands that many pixels outward at the corners, blue
three quarters of that inward, before mosaicking. Linear domain, AMaZE
(clean AMaZE on Kodak is 35.58):

| set   | CA at corners | uncorrected CPSNR | corrected CPSNR | gain    |
|-------|--------------:|------------------:|----------------:|--------:|
| Kodak | 1.5 px        | 31.40             | 32.65           | +1.3 dB |
| McM   | 1.5 px        | 29.88             | 30.72           | +0.8 dB |
| Kodak | 3.0 px        | 28.24             | 31.05           | +2.8 dB |
| McM   | 3.0 px        | 26.85             | 29.14           | +2.3 dB |

The bench images are small enough that these use the reduced fit, and the
synthetic aberration's bilinear resampling blurs red and blue in a way no
shift can undo, so the ceiling is below clean AMaZE. The gain is the
number that has to move. On the real files the fit is fourth-order from
1300 to 3600 tiles and finds 0.3 px (R5 II with the 100-500) to 1.1 px;
the orchids report 3.5 px at the corners, which on inspection is
out-of-focus purple fringing (axial, not lateral) read as shift; the
correction there reduces the fringes without artifacts. Center of frame
is untouched. Cost: about 0.25 s on 24 MP for two iterations.

**Not here:** manual red/blue sliders, and the reference's per-image fit
caching. Axial CA and purple fringing are a different tool.

### 13f. VNG4 and the dual demosaic (2026-09-05)

Two ports from RawTherapee, both GPL-3 with attribution in the file
headers. `develop::vng4` is Ingo Weyrich's `vng4_demosaic_RT.cc`, dcraw's
variable-number-of-gradients interpolation in four-color mode: the greens
on red rows and on blue rows are separate colors, so at a green site the
output is the mean of the sample and the other green interpolated from its
neighbors. That is why VNG4 does not maze on green imbalance or noise,
and why it is soft. `develop::dual` is `dual_demosaic_RT.cc` plus the blend
mask from `rt_algo.cc`: run the detail demosaic and VNG4, take L* of the
detail result, measure contrast as the root of the summed squared central
differences at one and two pixels, and blend by a sigmoid with its midpoint
at the contrast threshold, blurred with sigma 2. The threshold is either
fixed (RT's percent scale over 100; RT's default is 20) or measured: find
the flattest mid-tone tile, treat its contrast as noise, and take the
threshold at which one percent of that tile still counts as detail.

New methods `vng4`, `rcd-vng4`, `amaze-vng4`; `--dual-contrast auto|N` in
the CLI and the bench. AMaZE alone stays the default: on clean images the
dual is a wash, and on noisy ones the automatic threshold is the weak
point (below).

**Departures.** The reference's four-plane interpolated image is not
materialized; the linearly interpolated other-green at a neighbor is
computed on demand. The blur over the mask is an exact separable
Gaussian rather than the reference's recursive one. Nothing else.

**Measured.** The bench gained `--noise SIGMA`: Gaussian noise on the
mosaic with standard deviation SIGMA at mid grey, scaling as the root of
the signal like shot noise, plus a tenth of SIGMA as read noise; seeded
from the image name so runs repeat. Kodak CPSNR, sRGB domain when clean,
linear with noise:

| condition           | amaze | vng4  | amaze-vng4 auto | fixed threshold |
|---------------------|------:|------:|----------------:|----------------:|
| clean               | 39.22 | 35.35 | 39.23           |                 |
| noise 0.01          | 33.21 | 31.83 | 33.27           | 33.66 at 40     |
| noise 0.03          | 27.22 | 27.87 | 27.22           | 28.28 at 100    |

Zipper drops with the dual at every level (Kodak 6.80 to 6.49 clean, 28.4
to 27.7 at 0.01). RCD-VNG4 is the same story on RCD. On the real files the
measured threshold is 12 (orchids, 72% of the frame from VNG4), 18
(5M0A4160, 45%) and 31 (moon, 99.6%: black sky), which is the range RT
users see; crops of leaf detail and moon surface are indistinguishable
from AMaZE alone, and there are no seams. Cost: VNG4 is about 0.25 s per
24 MP on top of the base demosaic and the mask.

**The weak point.** At noise 0.03 the automatic threshold is zero: the
reference's tile search rejects every tile whose variance over mean
exceeds 8 in its units, on the sound reasoning that a tile that rough
might be texture, not noise, and a wrong threshold would smear texture.
So the dual quietly becomes AMaZE alone exactly when VNG4 alone would
beat it by 0.65 dB and a fixed threshold of 100 by 1.06 dB. A fixed
threshold we set closes the gap; what closes it properly is a
noise model, which the profiled noise reduction will bring (a sensor's
noise at a level gives the L* contrast the noise produces, and so the
threshold, with no tile search). Until then the dual is an opt-in.

**Not here:** RT's bilinear-blend variants (AMaZE+bilinear and so on),
the X-Trans dual, and darktable's version of the same idea. Noise
reduction is next; with it, the auto threshold above, and the dual may
become the default for high-ISO frames.

### 13g. Profiled noise reduction (2026-09-05)

Two modules. `develop::noise` is ours: a noise model `var = a x + b` per
channel (shot noise times ISO gain, plus read noise), estimated from the
frame itself. darktable ships a database of `a` and `b` per camera and
ISO measured from test shots; greycard has no database and no ISO on the
frame yet, so it measures. 16x16 blocks, per filter color the mean
squared residual of each sample against its four same-color neighbors
two pixels away (1.25 times the variance for white noise; texture only
adds), blocks binned by level evenly in the square root of the level,
the quietest quarter of each bin taken as noise-only and corrected for
the bias of taking the quietest quarter, and a weighted line through the
bins. Clipped blocks are left out. On synthetic frames with a known model
it is within 12 percent; on the bench, once the references' own grain is
accounted for (see below), within 7 percent.

`develop::denoise` is darktable's "denoise (profiled)" in wavelets mode
(GPL-3, attribution in the header): the generalized Anscombe transform
per channel turns the noise into unit white noise, an orthonormal
luma/chroma rotation, seven levels of the undecimated B3 spline à trous
wavelet with darktable's edge guard, BayesShrink soft thresholds per
band and channel, the residue added back, the rotation undone and
Mäkitalo and Foi's closed-form unbiased inverse of the transform. Runs
after the demosaic on white-balanced camera RGB, with the model
transformed for the gains (`a` scales with the gain, `b` with its
square). Off by default; `--denoise [--denoise-strength S]` in the CLI.

**Departures, and one that matters.** darktable uses one profile, the
green channel's, for all three channels and adapts it by the white
balance coefficients; that is why its luma/chroma matrix is
white-balance-adaptive, and its threshold constants (2.5 on the assumed
noise, 8 on the shrink, "because it seemed a little weak") are tuned
against that matrix, whose luma row also appears to carry the square of
the sum of the inverse gains where the intent reads as its reciprocal. A
faithful port of those constants onto exactly unit noise wiped whole
bands. So: per-channel transform from the per-channel model, the plain
orthonormal Y0U0V0 of Lebrun, Colom and Morel, the per-band noise
variance computed from the kernel rather than approximated as the 1D
norm to the power of the scale, and the textbook BayesShrink threshold
(band noise variance over band signal deviation) times one strength
multiplier chosen on the bench. darktable's newer transform with its
"preserve shadows" exponent and "bias correction" sliders is not
ported; the classic transform with the unbiased inverse has no knobs.
The transform is also rearranged so a channel whose model has `a` at
zero (read noise only, which the estimator produces on a channel with
nothing in it) stays finite in single precision: the first run put one
McMaster image at 2 dB from that.

**Measured.** Bench `--denoise [--denoise-strength S]` estimates the
model from the mosaic the demosaic sees and denoises after it. Linear
domain, AMaZE, CPSNR:

| noise at mid grey | set   | none  | S=1   | S=1.5 | S=2   |
|-------------------|-------|------:|------:|------:|------:|
| 0.02              | Kodak | 29.91 | 32.29 | 32.37 | 32.24 |
| 0.02              | McM   | 28.68 | 30.30 | 30.33 | 30.21 |
| 0.05              | Kodak | 23.35 | 27.26 | 27.83 | 28.06 |
| 0.05              | McM   | 23.43 | 26.26 | 26.38 | 26.34 |

Zipper falls by two thirds; ΔE improves everywhere. Strength 1.5 is the
default: never worse than 1, better at heavy noise. The references are
not clean: the estimator reads their own grain at about 0.02 at mid
grey, which is why at noise 0.01 denoising *costs* 0.4 dB on Kodak
(the grain is scored as signal, and the estimate of added-plus-own noise
is right), and why the bench is only trusted at 0.02 and above. Real
files measure 0.0023 to 0.0073 at mid grey (the moon frame the
noisiest); the sky behind the moon comes out clean, the moon's maria and
craters stay, fine crater speckle softens a little. Cost: about 1.6 s on
24 MP for seven scales, four full RGB buffers; tiling or the GPU fixes
both.

**And the question of AI noise reduction.** This stage and a learned
denoiser are different things and both wanted. This is deterministic,
fast enough, explainable, and produces the noise model; a learned
denoiser (DeepPRIME, Lightroom's Denoise, the raw-domain UNets) is
better at high ISO, expensive, needs a model file and an inference
runtime, and usually replaces demosaic and denoise together. The noise
model is what both need: the learned one wants the variance-stabilized
input so one network serves every ISO. So the plan is: this as the
always-available path, the learned one as a pluggable stage on the same
model, later.

**Next:** feed the model to the dual demosaic's threshold (§13f) instead
of RT's tile search; darktable's noise profile database as an optional
data file keyed by camera and ISO, once the frame carries ISO; tiling or
GPU for memory and time.

### 13h. The dual demosaic's threshold from the noise model (2026-09-05)

The weak point of §13f is closed. `DualContrast::Noise(model)` builds a
threshold per unit of L* from the noise model: for each lightness, the
model's deviation in Y (channels taken as fully correlated, since a
color-difference demosaic carries the sample's noise into the two
channels it fills in) times the slope of L* gives the noise in L*, and
the threshold is what RawTherapee's rule would return for a flat tile
with that noise: four central differences of Gaussian noise sum to
`2 sigma^2` times a chi-squared with four degrees of freedom, so the
mean blend at a candidate threshold is an integral over that density,
and the rule's "no more than one percent reads as detail, plus one
step" is evaluated on it directly. The mask then looks up each pixel's
threshold by its own lightness, so the shadows, noisier in L*, get a
higher one. A test checks the rule against the reference's tile search
on a flat noisy tile (0.39 against 0.36 at mid grey). In the pipeline
`Auto` now means this, with the frame's noise measured as for the
denoiser; `Tiles` keeps the reference's search; the CLI takes
`--dual-contrast auto|tiles|N`.

**Measured.** Linear domain, Kodak / McMaster CPSNR, `amaze-vng4`:

| noise | tile search   | noise model   | amaze alone   | vng4 alone    |
|-------|--------------:|--------------:|--------------:|--------------:|
| 0.01  | 33.27 / 31.23 | 33.79 / 31.46 | 33.21 / 31.16 | 31.83 / 30.36 |
| 0.03  | 27.22 / 26.58 | 28.28 / 27.29 | 27.22 / 26.58 | 27.87 / 27.06 |
| 0.05  | 23.35 / 23.43 | 24.22 / 24.12 | 23.35 / 23.43 |               |

At 0.03 the model reaches what the best fixed threshold found by hand
did (28.28 at 100), and beats both halves alone. On the real files the
thresholds agree with the tile search on the ISO 125 orchids (13 against
12) and rise above it on the noisier frames (26 against 18, 34 against
31); the frame that hands two thirds of itself to VNG4 shows no seam
and no lost detail where the two differ most.

**Default stays AMaZE alone.** On the clean sets the noise-model dual is
0.3 dB behind on Kodak and level on McMaster, because the references'
own grain (0.02 at mid grey, ten times a base-ISO raw) is read as noise
and smoothed, and the metric scores the grain as signal. The bench
cannot settle a default for real base-ISO files, whose measured noise is
far below anything the sets contain, so the conservative choice holds
until a policy by measured noise (dual above some sigma) is tested on
real high-ISO frames. That needs such frames; none in the test set
exceed ISO 1000.

### 13i. Parked: DNG interop, high-ISO sample (2026-09-05)

State at the pause. Commits through 66c48e7 (dual threshold from the
noise model). Everything in §13 through §13h is in and clean.

**DNG interop, first measurement.** Neither darktable nor RawTherapee is
installed; LibRaw's `dcraw_emu` is, and it is what many tools use. With
`dcraw_emu -w -o 1 -6 -T -W -q 3` on the source CR3 (5M0A8354) and on
greycard's linear DNG of it, the DNG render aligns with the CR3 render at
offset (14, 12) inside LibRaw's larger crop (6022x4024 against our
6000x4000) and is brighter by 15 percent in red and blue and 21 percent
in green, mean absolute difference 1.5 percent of full scale. Same
picture, wrong scale, and green off by more than red and blue. Suspects,
in order: LibRaw's white level for the CR3 (if it takes the nominal 14-bit
maximum rather than the camera's actual white, its CR3 render is darker,
and the fault is not ours), our `WhiteLevel` against the stored samples
in the linear DNG, and BaselineExposure. Green differing from red and
blue suggests something channel-dependent, which points at how the gains
are divided out before writing (§12) meeting LibRaw's own AsShotNeutral
handling. Not resolved. To reproduce: write the DNG with
`greycard develop X.CR3 --dng X.dng`, render both with the command above,
find the offset by minimizing the difference on a coarse grid, compare
per-channel means.

**High-ISO sample.** The test set tops out at ISO 1000, so the denoiser
and the dual demosaic's default policy have not been judged by eye on a
genuinely noisy frame. raw.pixls.us (CC0) lists Canon R6 Mark II samples
at file ids 6402 to 6409, R5 Mark II at 7881 to 7884, R6 Mark III at
8961 to 8964; the listing is `json/getrepository.php?set=all`, files at
`getfile.php/<id>/nice/<name>`, EXIF at `getfile.php/<id>/exif/<name>.exif.txt`.
The EXIF fetch attempted here returned nothing to a grep for ISO; check
the file format before filtering. Pick the highest ISO available.

**Still open, in order:** resolve the brightness discrepancy above; the
high-ISO frame, then decide whether the dual becomes the default above
some measured noise; tile the denoiser (four full RGB buffers); ISO on
`RawFrame` (rawler's metadata has it) and darktable's noise profile
database as an optional data file; darktable's segmentation highlights;
the GPU path last, scoped by what the editor needs.

### 13j. The DNG interop discrepancy was rawler's white level (2026-09-05)

Resolved. The 15 to 21 percent brightness gap in §13i was a wrong white
level, and the fault was neither ours nor LibRaw's rendering: rawler 0.8
reads the EOS R6 Mark II's white level from the wrong word of Canon's
ColorData block.

**Diagnosis.** LibRaw's `raw-identify -v` showed identical white balance
and matrices for the CR3 and our DNG, so only levels were left. A small
program against libraw printed the levels it uses: black 512, maximum
16383 (the 14-bit ceiling), Canon `SpecularWhiteLevel` 14888,
`NormalWhiteLevel` 13535. greycard, through rawler, had white 12735. A
histogram of the raw samples inside the crop settled which was real: a
smooth distribution right up to a pile-up at 16383, tens of thousands of
samples per channel between 12735 and 16383, no feature at 12735 or
14888 at all. The sensor clips at 16383; nothing in the file saturates
at 12735.

rawler's `cr2/colordata.rs` maps ColorData sub-version 48 (R6 Mark II,
R7, R8, R10, R50) to white levels at words 0x281 and 0x282. exiftool's
`Canon.pm` (`ColorData11`, which covers sub-versions 34 and 48 alike)
and LibRaw's `canon.cpp` (`0x0069 + 0x0217` for both 3973 and 3778
word blocks) put `NormalWhiteLevel` at 0x280, `SpecularWhiteLevel` at
0x281 and `LinearityUpperMargin` at 0x282. rawler's entry for 34 is
right and its entry for 48 is one word late: the 12735 it reports as
the specular white is the linearity margin. The R5 Mark II (sub-version
64) is unaffected, which is why the moon DNG carried 14888.

**Verification.** greycard built against a locally patched rawler
(`cargo build --config 'patch.crates-io.rawler.path=...'`, nothing
committed) reads 14888 and writes a DNG whose LibRaw render, fitted
against LibRaw's CR3 render on unclipped pixels, has per-channel slopes
1.098, 1.103 and 1.081 where the unpatched DNG had 1.291, 1.298 and
1.271. The prediction from the levels alone is (16383 - 511) /
(14888 - 512) = 1.104 for the patched file and 1.299 for the original.
The 10 percent that remains is LibRaw's choice of the 14-bit ceiling
over Canon's declared specular white, which LibRaw itself reports as
the linearity limit but does not use as its maximum. Ours is the level
Canon declares.

**What the bug cost.** Every R6 Mark II frame in the test set (four of
six files) was developed against a range 15 percent too small: exposure
0.22 stop high, and every sample between 12735 and the real clip was
thrown away as blown. 0.84 percent of 5M0A8354's samples, about five
percent of 5M0A3976's. The highlight reconstruction in §13d was voting
on false clips. The noise figures for these files in §13g are in units
of the wrong range and shrink by 12223 / 14376 = 0.85 once corrected;
the bench, which runs on PNG references, is untouched.

**Canon's white levels are per file.** The rawdb samples show why the
values have to be read, not tabulated: R6 Mark II 14008 at ISO 100 and
14888 at ISO 125 to 500; R7 13036 at ISO 100 and 13660 at ISO 32000;
R8 14008; R50 14338 at ISO 800. All five bodies write 12735 as the
margin, which is why every one of them looked the same through rawler.

**What greycard now reports.** `RawFrame::white_check` compares every
sample in the crop against the white level declared for its position
and gives the brightest sample and the count above; `greycard info`
prints it. It reports, it does not judge: Canon declares its white
below the ADC ceiling on purpose (the specular white is where linearity
ends), so a corrected R6 Mark II file still has 0.66 percent of its
samples above 14888, and the overexposed R7 sample at ISO 32000 has 20
percent above its 13660. A first version warned when more than one in
ten thousand samples sat above the level; it fired on every Canon frame
with a blown highlight and was taken out. The signature of the bug is
not "samples above the level" but "a level nobody else agrees with":
LibRaw's, exiftool's, and the histogram's. Those are the checks.

**Upstream.** The fix is one line in rawler's `colordata.rs` (sub-version
48 to 0x280 and 0x281). rawler's rawdb tests compare the analyzer's
output with checked-in yaml files exactly, so the 44 yaml files for the
five bodies must be regenerated from the samples, which is 2.8 GB from
rawdb.dnglab.org; that is being done here with the patched `dnglab
analyze`, and every file so far differs only in its `whitelevels` line.
The PR goes up once we've said
so. Until it is released, greycard can consume the branch through
`[patch.crates-io]`; that is a pointer at the upstream contribution, not
a fork, and is removed when a rawler release carries the fix.

**High-ISO samples, found.** The rawdb has R7 and R10 frames at ISO
32000 (CC0), downloaded alongside the above. They serve the open
high-ISO item in §13i.

### 13k. A first look at ISO 32000, and a chroma strength for the denoiser (2026-09-05)

The rawdb's EOS R7 frame at ISO 32000 (1/8000 at f/8: a sunlit
building, overexposed, a fifth of its samples clipped) is the noisiest
file on hand. Developed against the corrected white level (13660), the
noise model reads sigma at mid grey R 0.036, G 0.047, B 0.036, five to
ten times the test set's ISO 1000 file. The dual demosaic's threshold
from that model is 101, above RawTherapee's slider range, and 76
percent of the frame comes from VNG4. In the crops the dual is quieter
than AMaZE in the dark window interiors, with less of AMaZE's fine maze,
and there is nothing to lose at this noise: the policy question in §13i
(does the dual become the default above some noise) has its first data
point, and the answer there is yes; the threshold below which AMaZE
alone should stay the default is still to be set, on frames between
ISO 1000 and this.

The denoiser at its bench-calibrated strength 1.5 takes out most of
the luminance grain and leaves two things. Coarse color blotches,
purple and green patches several pixels across, and isolated dark and
light specks a pixel or two wide. The blotches are the easy one:
color noise has no fine detail worth keeping, so the two chrominance
channels can be shrunk harder than luminance. `DenoiseOptions::chroma`
multiplies the U and V thresholds; the bench on Kodak and McMaster with
synthetic noise at 0.03 and 0.05, AMaZE, strength 1.5:

| noise | chroma | Kodak CPSNR | Kodak dE | Kodak dab | McM CPSNR | McM dE | McM dab |
|------:|-------:|------------:|---------:|----------:|----------:|-------:|--------:|
| 0.05  | 1      | 27.23       | 4.76     | 3.03      | 27.44     | 6.02   | 4.83    |
| 0.05  | 2      | 27.29       | 4.57     | 2.76      | 27.41     | 6.00   | 4.77    |
| 0.05  | 4      | 27.28       | 4.56     | 2.72      | 27.32     | 6.09   | 4.85    |
| 0.03  | 1      | 30.22       | 3.54     | 2.36      | 30.01     | 4.59   | 3.78    |
| 0.03  | 2      | 30.24       | 3.49     | 2.27      | 29.94     | 4.62   | 3.80    |
| 0.03  | 4      | 30.21       | 3.52     | 2.30      | 29.83     | 4.71   | 3.88    |

Chroma 2 lowers the color error (dab, the a*b* part of dE) by nine
percent on Kodak for no luminance cost; 4 gains nothing more on Kodak
and starts to cost McMaster, whose saturated fine detail is exactly
what a hard chroma shrink eats. `DEFAULT_CHROMA` is 2. By eye on the
R7 crops, 2 removes most of the blotching and 4 the rest, at a slight
neutral cast; the benchmark's caution is the right default and the
flag is there.

The specks are the harder one and are not addressed. They are not hot
pixels: a count of samples exceeding all eight same-color neighbors
by 1500 raw units finds 0.09 percent on this frame, which is what the
tail of noise with sigma near 900 raw units gives, against 0.0004
percent on the ISO 320 file. They are noise the finest scale's
threshold lets through. BayesShrink sets each band's threshold from
one signal variance for the whole band, and a frame full of clipped
edges has a large one, so flat areas get a threshold sized for edges.
The fix is the standard one: estimate the signal variance in a local
window rather than globally, so the threshold rises in flat areas and
falls at edges. That is the next denoiser step, ahead of tiling.

### 13l. BayesShrink with a local signal estimate (2026-09-05)

The threshold now comes from the band's variance around each pixel
rather than over the whole band. `LocalVariance` takes the mean square
of the detail coefficients over tiles of 8 times 2^scale pixels (the à
trous coefficients are correlated over the filter's spacing, so the
window grows with it) and interpolates bilinearly between tile centers,
so thresholds do not step at tile edges. The signal variance is what
clears a noise floor of `sb2 (1 + 2 sqrt(2/N))`: a tile of pure noise
measures the noise variance give or take `sqrt(2/N)` of it, and without
that margin half the flat tiles would get a finite threshold drawn from
the estimate's own scatter. Flat areas therefore get an infinite
threshold and lose every coefficient; edges and texture keep theirs.
This is the spatially adaptive form of BayesShrink (Chang, Yu and
Vetterli, 2000), not something darktable does.

Bench, AMaZE, strength 1.5, chroma 2, Kodak / McMaster CPSNR:

| noise | global rule (§13k) | local rule | gain |
|------:|-------------------:|-----------:|-----:|
| 0.01  | 35.41 / 33.52      | 35.58 / 33.64 | +0.17 / +0.12 |
| 0.03  | 30.24 / 29.94      | 30.89 / 30.32 | +0.65 / +0.38 |
| 0.05  | 27.29 / 27.41      | 27.96 / 27.66 | +0.67 / +0.25 |

The zipper score drops with it (Kodak at 0.05: 33 to 25 percent), which
is the flat areas coming out clean. Strength stays at 1.5: 2.5 is
better by 0.6 dB at noise 0.05 and worse by 0.7 dB at 0.01, and the
files this tool sees are at 0.002 to 0.007. A strength that follows the
measured noise is the obvious next knob; it is not in yet. The chroma
multiplier was re-checked under the local rule and 2 still lowers the
color error on Kodak (dab 2.96 to 2.71 at 0.05) for a few hundredths
of a dB on McMaster; it stays.

**The specks, not fixed.** The ISO 32000 crops still carry isolated
single-pixel dots, dark, light and colored. Three hypotheses were
tested and rejected, each by a render of the frame. That the edge
guard in the à trous blur passes outliers as edges: the guard turned
off changes nothing. That they come through the coarse residue: with
every detail band zeroed (strength 1000) the residue is clean, so they
are detail coefficients that survive their threshold. That the outlier
inflates its own tile's variance estimate: capping each coefficient's
contribution at (3 sigma)^2 changes nothing in the crop and costs 0.5
dB at noise 0.01, where real detail is what gets capped, so it is out.
They are what remains when a soft threshold near 3 sigma meets 24
million samples times three channels times seven bands of Gaussian
noise: the tail. Textured tiles keep finite thresholds by design, and
a 6 sigma coefficient there leaves as a 3 sigma dot on a smoothed
background. Killing them by threshold means over-smoothing, which the
low-noise bench refuses. The answer is a different estimator for this
regime (non-local means, or the AI denoiser in §13g), not more of
this one.

### 13m. The dual demosaic becomes the default (2026-09-05)

The policy question from §13i and §13k, settled on the bench. The
rawdb samples give the noise model's reading at every ISO on hand: R6
Mark II 0.002 at ISO 100 and 0.005 at 320; R7 0.005 at 100, 0.012 at
800, 0.04 at 32000; R50 0.004, 0.008, 0.04 at the same three. Real
files at base ISO sit at 0.002 to 0.005, where the model's dual
threshold comes out at 12 to 23, and 60 to 90 percent of every frame,
noisy or clean, goes to VNG4: that is the flat area of a photograph,
which is most of it.

So the comparison that matters is AMaZE against the dual at the
thresholds real files get. Bench, AMaZE against AMaZE+VNG4, Kodak /
McMaster CPSNR, no denoising:

| noise | threshold | AMaZE | AMaZE+VNG4 | gain |
|------:|----------:|------:|-----------:|-----:|
| clean | fixed 13  | 39.22 / 35.96 | 39.27 / 36.02 | +0.05 / +0.06 |
| 0.005 | model     | 37.39 / 35.15 | 37.48 / 35.31 | +0.09 / +0.16 |
| 0.01  | model     | 34.58 / 33.50 | 35.15 / 33.94 | +0.57 / +0.44 |
| 0.02  | model     | 30.15 / 30.19 | 31.23 / 31.03 | +1.08 / +0.84 |

The dual is never behind, the zipper score drops at every level, and
the gain grows with the noise. The clean-bench loss recorded in §13h
(0.33 dB) was the noise model reading the references' own grain as
noise and setting a threshold near 40; at 13, which is what a base-ISO
file measures, the dual is a hair ahead of AMaZE even on clean
references. `DemosaicMethod::AmazeVng4` with `DualContrast::Auto` is
the default in the engine, the CLI and the DNG path. Cost: about
twice AMaZE's time, 550 against 250 ms per megapixel on the bench
machine; the noise estimate it needs is 40 ms. A 24 MP R6 Mark II file
develops in 1.4 s with everything on, 2.9 s with the denoiser as well.

Housekeeping with it: the denoiser now holds three full-frame buffers
(the caller's, the current level and its coarse), not five; at 24 MP
that is 860 MB rather than 1.4 GB. Tiling, the real fix, is still on
the list.

### 13n. The denoiser's strength follows the noise (2026-09-05)

§13l left the strength at 1.5 because 2.5 won at noise 0.05 and lost
at 0.01. `Strength::Auto`, now the default, draws it from the model:
`STRENGTH_LOW` 1.5 up to a green-channel sigma of `SIGMA_LOW` 0.03 at
mid grey, `STRENGTH_HIGH` 2.5 from `SIGMA_HIGH` 0.05, a straight line
between. `Strength::Fixed` and `--denoise-strength N` keep the old
behavior; `--denoise-strength auto` is the default on both binaries.

Bench, AMaZE, chroma 2, Kodak / McMaster CPSNR:

| noise | fixed 1.5 | fixed 2.5 | auto |
|------:|----------:|----------:|-----:|
| 0.01  | 35.58 / 33.64 | 34.90 / 33.27 | 35.51 / 33.54 |
| 0.03  | 30.89 / 30.32 | 30.93 / 30.37 | 30.91 / 30.35 |
| 0.05  | 27.96 / 27.66 | 28.55 / 28.05 | 28.55 / 28.05 |

Auto takes the better column at 0.05 and 0.03 and gives up 0.07 dB at
0.01. That last is the bench's own grain: the references carry about
0.02 of noise before any is added, so at "0.01" the estimator reads
0.024 on average and above 0.03 on some images, which puts them a
little way up the ramp. A first version with the ramp from 0.02 lost
0.19 dB there for the same reason. Real files below about ISO 6400
measure under 0.03 and get exactly 1.5; the R7 and R50 at ISO 32000
measure 0.046 and get 2.3.

**Upstream, done (2026-09-05, later).** The rawler fix is
[dnglab/dnglab#840](https://github.com/dnglab/dnglab/pull/840), from
a fork, branch `canon-colordata-48-white-level`: the
one-line offset change and the 44 rawdb yaml files regenerated with the
patched `dnglab analyze`. The workspace `Cargo.toml` carries a
`[patch.crates-io]` entry at that commit until a rawler release has the
fix; every R6 Mark II file now reads its white as 14888 and the
`brightest` line in `info` shows 0.66 percent of samples above it,
which is Canon's own margin, not ours.

### 13o. Segment-based highlight reconstruction, ported and left opt-in (2026-09-05)

darktable's "segmentation based" mode is a second pass over the opposed
result: each color plane is reduced to 3x3 blocks in cube-root space,
clipped blocks are dilated (the "combine" radius, 2) and flood-filled
into segments with their unclipped border blocks attached, and each
segment looks for its smoothest unclipped block, whose reading minus its
opposed average becomes that segment's chrominance in place of the
frame's one offset. `develop::segments` ports the plane reduction, the
morphology and the scanline flood fill from `segmentation.c`, the
candidate weighting and the correction from `segbased.c`, with tests for
the flood fill, the closing and a two-lamp scene where per-segment
offsets cut the reconstruction error by more than half. Not ported: the
"rebuild" modes, which invent luminance inside regions where all three
channels are clipped from border gradients and a distance transform,
off by default in darktable; and the mask views. The reference's
candidate weight carries a factor that is always 1, kept as found and
noted in the module.

**What it does on real files: nothing, at darktable's defaults.** The
candidate weight is `1 - 10 sqrt(std)` of the block plane over a
21-block cross, and a candidate needs more than 0.6 of it, which is a
standard deviation under 0.0016 in cube-root units: a spot flatter than
photon noise at base ISO makes a 3x3 block mean. On the three R6 Mark
II files with clipped highlights every segment's best spot scored 0.2
to 0.47, so no segment got a candidate and the output is the opposed
output. On the bench at +1 and +2 EV the numbers are the opposed
numbers to the hundredth of a dB (McMaster loses 0.08 at +1 EV where a
few candidates are found). With the threshold loosened to 0.7 or 1.0
(`--candidating`) the candle file gets 2 to 21 candidates and the
change is a thin ring at the edge of each clipped blob, a slight shift
of the glow's tone, not clearly better by eye.

So the mode is there (`--highlights segments`, `--combine`,
`--candidating`) and opposed stays the default. What darktable users
value the mode for is mostly its rebuild option on fully blown areas,
which is the part not ported; that is a separate and larger piece
(distance transform, gradient propagation, Poisson noise) for when a
frame shows the need.

### 13p. Where the memory went, and getting it back (2026-09-05)

The open item said "tile the denoiser, three full RGB frames". Measured
before touching it, resident memory over a 24 MP develop (`5M0A8354`):

| stage                      | RSS after | high-water |
|----------------------------|----------:|-----------:|
| balanced samples           |    178 MB |     273 MB |
| CA correction              |    316 MB |     481 MB |
| AMaZE                      |    838 MB |    1118 MB |
| VNG4 in the dual           |   1213 MB |    1306 MB |
| denoise                    |    837 MB |    1399 MB |

The denoiser was not the peak. AMaZE collected every tile's output into
a second frame before copying it over the bilinear base, then made a
third whole frame for its three-pixel bilinear border; the freed tile
outputs stayed parked in the allocator's arenas (370 MB after AMaZE
returned, below the mmap threshold so never given back). The dual pass
then held VNG4's whole frame and its green plane to blend once. The
denoiser's two extra frames fitted in what AMaZE had freed and added
90 MB to the peak.

**Demosaic.** AMaZE now copies each tile into the output as it finishes,
under a lock held for the copy alone, and fills the border per pixel
(`bilinear_pixel`, shared with VNG4). VNG4 runs in bands of 64 rows
(`Vng4::rows`), so the dual demosaic makes each band and blends it in;
the standalone VNG4 is the same bands into one frame. Bytes out are
identical to before. Peak without denoise: 1274 → 856 MB, and a little
faster (1.13 → 1.01 s).

A first version streamed AMaZE's tiles through a bounded channel to a
copier on the calling thread. It worked from the CLI and deadlocked in
the bench, which develops images in parallel from inside rayon workers:
every worker blocked on its receive while the tile jobs that would feed
it sat in the same pool. Nothing in the engine may park a worker on
work that needs a worker; nested rayon is fine, channels and condvars
are not.

**Denoiser.** Two changes, and the second is the one worth knowing
about. The à trous chain now runs in the caller's buffer a band of 256
rows at a time: each band's coarse goes to a band buffer and is
committed over the input rows only after the next band has been
blurred, since the blur at spacing `2^s` reads `2^(s+1)` rows beyond
its own. That drops one full frame. The other full frame, the
accumulated shrunk detail, stays.

The local variance (§13l) was measured on every coefficient of every
band, which needed the chain's full-frame buffers before any shrinking
could start, and is what made exact tiling impractical: the coarsest
tiles are 512 pixels and interpolate across neighbors, four times the
wavelet's reach. It is now measured first, on a decimated pyramid: each
level is the previous one blurred with the 5-tap filter and taken every
other pixel, which is the unguarded à trous chain evaluated on the
lattice of its own scale. Every tile at every scale is then 64
critically sampled coefficients (before: 64·4^s correlated ones, and a
noise-floor margin that shrank with s as if they were independent).
The whole pyramid costs a third of one plain separable blur and a
quarter of the frame, briefly. The guard is left out of the
measurement; it only bites at strong edges, where the variance is large
and the threshold near zero either way.

Bench, AMaZE, auto strength, chroma 2, mean PSNR / mean ΔE:

| noise | Kodak before   | Kodak after    | McM before     | McM after      |
|------:|----------------|----------------|----------------|----------------|
| 0.01  | 35.51 / 2.33   | 35.59 / 2.28   | 33.54 / 3.54   | 33.64 / 3.46   |
| 0.03  | 30.88 / 3.32   | 30.87 / 3.32   | 30.33 / 4.52   | 30.35 / 4.49   |
| 0.05  | 28.55 / 4.11   | 28.54 / 4.12   | 28.05 / 5.81   | 28.09 / 5.79   |

Neutral to slightly better, and on the R7 ISO 32000 frame (§13k) the
window and the graffiti crops have visibly fewer of the specks §13l
could not remove: the uniform 1.35σ² floor at the coarser scales
removes low-frequency grain the old, overconfident floor let through.
Same speed (2.66 s for 24 MP with denoise). Peak with denoise:
1366 → 955 MB at 24 MP, 2421 → 1710 MB at 45 MP.

**What is left in the peak.** The remaining 856 MB at 24 MP is the
mosaic (96 MB) plus one RGB frame (288 MB), the blend mask and the
lightness plane (96 MB each, briefly), the raw frame and its normalized
copy before the crop, and AMaZE's per-thread scratch. The denoiser's
one extra frame now sits just above that. Exact tiling of the
denoiser is now straightforward, since the variance grids are global
and the chain per tile only needs the wavelet's reach as margin, but it
costs 1.5–2× the time for tiles of 1–2k pixels and is not worth it
until a memory budget says so.

### 13q. ISO on the frame; darktable's noise database is not ours to use (2026-09-05)

`RawFrame::iso` comes from rawler's EXIF (ISOSpeedRatings, else
ISOSpeed), on every decode path, and `info` prints it. The engine keeps
measuring the noise it acts on from the frame; the ISO is for consumers
and reports, and for whatever per-ISO policy turns out to want it.

The other half of the item, darktable's `noiseprofiles.json` as an
optional data file keyed by camera and ISO, is dropped after reading
the numbers. Its `a` and `b` are not variance per normalized raw unit.
For the EOS R7 at ISO 32000 darktable has green `a` = 5.2e-4; the frame
measures 1.05e-2, twenty times more, and that number is the physical one
(about 95 electrons at white, and a shot noise at mid grey that matches
the frame's). At ISO 100 the ratio is nearer forty. The profiles are
fitted on the output of darktable's own pipeline, demosaiced and
white-balanced (their red and blue `a` sit above green by about the
squared white balance gains, where the raw's sit below), so they carry
that pipeline's averaging and scaling, and not by a constant. Without
running darktable's profiling pipeline there is no mapping into the
model here, and the per-frame measurement is the better number anyway:
it sees this sensor at this temperature with this white balance. The
database stays a cross-check someone could do by hand, not a data file.

### 13r. Non-local means, and the hybrid that is now the default (2026-09-05)

The specks §13l could not remove are gone. `develop::nlm` ports the
non-local means of darktable's `nlmeans_core.c` as `denoiseprofile.c`
drives it: for every offset in a search window, the sum over a patch of
the squared differences between the pixel's surroundings and the
offset's, turned into a weight `exp2(-max(0, d * norm - 2))`, the
offset's pixel added with that weight. The sliding column sums that
make it cost patches times offsets rather than patches times offsets
times patch area are the reference's; the tiling is ours (96-pixel
tiles, a patch margin each, parallel over tile rows and within them).
It runs on the same stabilized values as the wavelets, 3x3 patches, a
dense 15x15 window, and one extra frame. 24 MP in about 1.4 s.

**The reference's operating point is wrong for this engine, by a factor
of twenty.** With its norm, 0.045 over the patch area, any two patches
within about four sigmas per channel of each other average with full
weight; the result is a wash by eye (the ISO 32000 frame lost every
edge) and the benchmark's worst denoiser at every noise level, 4 to 6
dB under the wavelets, whatever the patch size or search spread. The
strength slider in darktable scales the same thing through its
profile, so its users presumably turn it down. Sweeping the scale:

| noise | wavelets       | NLM 0.045      | NLM x4         | NLM x10        | NLM x20 (=1)   | NLM x40        |
|------:|----------------|----------------|----------------|----------------|----------------|----------------|
| 0.01  | 35.59 / 33.64  | 27.37 / 27.88  | 32.66 / 32.81  | 34.80 / 34.16  | 35.70 / 34.54  | 35.83 / 34.48  |
| 0.03  | 30.87 / 30.35  | 22.79 / 22.49  | 29.29 / 29.98  | 31.33 / 31.62  | 31.43 / 31.32  | 30.35 / 30.23  |
| 0.05  | 28.54 / 28.09  | 20.54 / 19.69  | 27.33 / 27.51  | 29.18 / 29.20  | 28.74 / 28.64  | 26.98 / 27.05  |

Mean PSNR, Kodak / McMaster, AMaZE, 3x3 patches, dense search 7. The
port's `NORM` is 0.9, twenty times the reference's, and its strength 1
is that column; strength divides the norm, so more smooths more, as
for the wavelets. Two more things the sweep settled: the reference's
patch growth with ISO (17x17 by ISO 32000) is wrong here, since a
patch that size shifted a pixel across an edge still matches on most
of its area and gets full weight, which is what softened it, so
patches stay 3x3 and grow to 5x5 only above a sigma of 0.04 (+0.3 dB
at 0.05); and its scattering of the search window for speed costs 1
to 2 dB against the dense window at the same offset count, so the
dense window is the automatic choice.

**Color.** At strength 1 the means beat the wavelets on PSNR and
median color error everywhere, but the mean color error, which is
the outliers, is worse from noise 0.03 up (16 vs 14 at 0.03, 29 vs 15
at 0.05): blotches of chrominance wider than the search window can
average away, visible on the ISO 32000 frame as magenta and green
patches in the dark strokes. Strength 2 halves that (8.0 / 12.0 at
0.03, 10.6 / 17.6 at 0.05) and keeps the PSNR; at 0.01 it costs 0.9
dB. So the automatic strength ramps 1 to 2 over a green sigma of
0.025 to 0.045 at mid grey as the estimator reads it (the bench's
quietest images read 0.024 from their own grain, §13n's lesson again).

**The hybrid.** The means clean the fine grain and the specks; the
wavelets see the coarse mottling a 15-pixel window cannot. The default
denoiser is now both: the means, then the wavelet chain with scales 0
and 1 unshrunk (`--hybrid-from 2`; from 1 loses PSNR at 0.05, from 3
gains nothing) at the wavelets' own strength ramp.

| noise | wavelets              | means (ramp)          | hybrid (default)      |
|------:|-----------------------|-----------------------|-----------------------|
| 0.01  | 35.59 / 33.64 · 6.3 / 11.2 | 35.70 / 34.54 · 8.1 / 11.9 | 35.48 / 34.27 · 7.8 / 11.8 |
| 0.03  | 30.87 / 30.35 · 14.4 / 17.1 | 31.33 / 31.62 · 8.0 / 12.0 | 31.11 / 31.36 · 9.2 / 13.1 |
| 0.05  | 28.54 / 28.09 · 15.1 / 21.7 | 29.18 / 29.20 · 10.6 / 17.6 | 28.65 / 28.63 · 7.4 / 16.8 |

Mean PSNR, then mean color error, Kodak / McMaster; the means column
at strength 1, 2, 2, the hybrid as shipped, with the ramp (which the
bench's quietest images, reading 0.024 to 0.03, just enter). On the
numbers the means alone edge the hybrid at 0.03 and 0.05 by a quarter
to half a dB of PSNR and the hybrid wins the mean color error at 0.05. By eye on the ISO
32000 frame the hybrid is not close: the means alone leave the color
blotches in the graffiti's dark stroke and on the window glass, the
hybrid's stroke is one clean grey and the glass one clean green, no
specks, edges smooth. Base ISO is indistinguishable across the three.
That is the default; `--denoise-method nlm` and `wavelets` remain, with
`--nlm-patch`, `--nlm-search`, `--nlm-scatter` and `--hybrid-from`.
Cost: 24 MP develops with denoise in 4.3 s (2.7 with either alone),
peak memory unchanged at just over a gigabyte.

What is left at ISO 32000 is a slight waviness along hard edges that
is the means' doing (3x3 patches at four sigmas of noise mis-match
now and then) and low-frequency luminance mottling the wavelets'
coarse scales judge to be signal. Both are a long way from the specks.

### 13s. Hot pixels: ported, opt-in, and why it stays off (2026-09-05)

The denoisers cannot remove a hot photosite by design: nothing around
it agrees with it, so non-local means gives every offset a vanishing
weight and returns the pixel, and the wavelets' soft threshold takes a
few sigmas off a coefficient of hundreds. Counting on the raw dumps
(same-color neighbors two away, all eight): the R7 at ISO 100 has 16
photosites more than 2000 raw units, about 25 sigma, above all of
theirs, and the three R6 Mark II frames 4 to 46. Real defects, a few
dozen a frame.

`develop::hotpixels` follows darktable's `hotpixels.c` (compare with
the same-color neighbors two away, replace a hot one with the
brightest of them), against eight neighbors rather than four, and
with two tests instead of its fixed level: the photosite must be more
than `sigmas` (6) of the frame's measured noise beyond its brightest
neighbor, and more than `ratio` times it. The sigma test alone is
what an engine with a noise model would write, and it is wrong at base
ISO: with the noise at half a percent of level, ordinary fine texture
clears six sigma above all eight neighbors constantly, 6000 to 83000
photosites a frame. The ratio is what holds it: at 1.5 the R7 frame
gives its 16, but a portrait gives 28000 and a candle frame 1800; at 3,
0 to 66 across seven frames.

Then the crops. Of the portrait's candidates at ratio 2, one was a
dark speck in fur, and one was the catchlight in the subject's eye,
which the repair turned yellow (the flagged photosite was one color of
a white point two photosites across). A star, a specular glint, a
catchlight, a speck of texture: all at most two photosites across, all
hot to this test and to every test of its kind, darktable's and
RawTherapee's included, which is why both ship theirs off. So does
this one: `--hot-pixels`, `--hot-sigmas`, `--hot-ratio`, off by
default and documented as not safe to leave on.

What separates a defect from a picture is that the defect is in the
same place in every frame. The design that gets this right is a
per-camera defect map, built once from dark frames (or from several
frames of anything, by intersection) and applied without judgment on
the mosaic; RawTherapee's "bad pixel map" is that. It needs the frames
to exist and a place to keep the map, so it waits for the editor.

### 13t. The central pixel weight in the non-local means (2026-09-06)

The one parameter of darktable's non-local means left out of the port
in §13r was its central pixel weight, tried now as the remedy for the
edge waviness noted there. The reference adds the center pair's own
squared difference to the patch sum, scaled up to the patch's pixel
count and multiplied by the weight, and divides the whole by one plus
the weight: at 1 the center pixel and the patch count the same, at 0
the patches alone are compared. `NlmOptions::center_weight`,
`--nlm-center`, and the reference's default of 0.1 is now this one.

Bench, AMaZE, linear domain, hybrid at its automatic strengths, CPSNR
then mean color error, Kodak / McMaster:

| center weight | noise 0.01                | 0.03                      | 0.05                      |
|--------------:|---------------------------|---------------------------|---------------------------|
| 0             | 33.50 / 31.79 · 2.90 / 3.99 | 30.57 / 28.84 · 3.79 / 5.38 | 27.88 / 25.84 · 4.86 / 7.35 |
| 0.1           | 33.68 / 31.82 · 2.86 / 3.97 | 30.69 / 28.86 · 3.75 / 5.34 | 28.23 / 26.00 · 4.77 / 7.25 |
| 0.3           |                           | 30.65 / 28.77 · 3.76 / 5.34 | 28.26 / 26.00 · 4.77 / 7.22 |
| 1             |                           | 30.43 / 28.56 · 3.80 / 5.37 | 27.93 / 25.78 · 4.88 / 7.27 |
| 3             |                           | 30.24 / 28.40 · 3.84 / 5.42 | 27.64 / 25.57 · 4.97 / 7.36 |

(These are linear-domain scores and sit a dB or two under the
gamma-domain tables of §13r; the comparison within the table is what
matters.) A small, steady gain at 0.1 through 0.3 at every noise
level, a third of a dB on Kodak at 0.05, and a loss from 1 up, where
the weight does what one would expect: a pixel that must match its
partner's own value keeps its own noise, since the partners that agree
with it share it, and the flat areas of the ISO 32000 frame grow a fine
grain again by 1 and clearly by 3.

By eye it is not the remedy for the waviness. At 400 percent on the
window frame edge of the R7 frame, 0, 0.1, 0.2 and 0.3 show the same
wave in the same places; the higher weights sharpen the edge's
definition a little and change its line not at all. The wave is a
patch-scale effect, a 5x5 patch at four sigmas of noise choosing a
partner a pixel across the edge now and then, and the center pair's
one difference does not decide that choice. So the setting is taken
for its benchmark gain, at the reference's value, and the waviness
stays on the list; the honest candidates for it are a smaller patch at
that noise (the bench preferred 5x5 there in §13r, by a margin that
may not survive a by-eye check) or a second pass of the means on its
own output, which the reference does not do either.

**The wave, attributed.** With the weight ruled out, the same edge at
400 percent under each suspect in turn: 3x3 patches instead of 5x5,
the same wave; AMaZE alone under the dual, the same wave; the means
alone, the same wave; the wavelets alone, no wave, a softer edge. So
it is the means', not the demosaic's, and not the patch's shape. The
strength is what moves it: at 1 the edge is crisp with a trace of the
wave, at 1.5 between, at the automatic 2 the wave is at its fullest.
That is the mechanism one would write down: at strength 2 the weight's
floor admits a partner whose mean squared difference is twice what it
admits at 1, and on a soft edge (this frame is at f/8 on a 32 MP
APS-C sensor, the edge two pixels wide before the noise) a partner one
pixel across the edge is admitted at full weight, so the edge's
position averages over where the noise puts it. The bench put the ramp
at 2 for its color error at a sigma of 0.05 (§13r), and one crop does
not overturn a bench; but the price of that choice now has a name, and
the remedy would be a strength that depends on where a pixel is (lower
on an edge, from the same local variance the wavelets already read)
rather than a lower ramp for the whole frame. Listed, not done.

**Measured, and the attribution above withdrawn.** The eye is not a
good judge of a wavy line at 400 percent, so the position of the
window frame's edge, of a thin dark line beside it, and of a thin
horizontal line in the graffiti crop were measured per row (or column)
of each render, a fitted straight line removed, and two numbers taken:
the standard deviation of what remains, which is the low-frequency
wander, and the root mean square of the row-to-row change, which is the
noise's part (`scripts/line-wander.py`). Thin vertical line, then the
horizontal one, wander / step, in pixels:

| render                     | vertical      | horizontal    |
|----------------------------|---------------|---------------|
| no denoise                 | 1.01 / 0.75   | 1.20 / 0.63   |
| wavelets alone             | 1.23 / 0.37   | 1.22 / 0.33   |
| means alone                | 1.24 / 0.31   | 1.20 / 0.24   |
| hybrid (default)           | 1.34 / 0.31   | 1.15 / 0.22   |
| hybrid, center weight 0    | 1.37 / 0.28   | 1.16 / 0.19   |
| hybrid, center weight 1    | 1.28 / 0.51   | 1.16 / 0.41   |
| hybrid, 3x3 patches        | 1.36 / 0.41   | 1.17 / 0.27   |
| hybrid, means strength 1   | 1.25 / 0.42   | 1.17 / 0.31   |
| hybrid, means strength 1.5 | 1.28 / 0.35   | 1.17 / 0.26   |
| hybrid over AMaZE alone    | 1.33 / 0.50   | 1.15 / 0.29   |

Two things the eye got wrong. The wander is in the picture: the
undenoised render has it at a pixel already, and no denoiser adds
more than a fraction of a pixel to it (the hybrid a tenth or so on the
vertical line, nothing on the horizontal one). It is a painted window
frame at a resolution where a pixel is a fraction of a millimeter of
wood. And the step, which is what the denoiser is answerable for, is
smallest at the default: strength 1 is rougher than 2, not crisper; the
center weight at 1 doubles it; 3x3 patches are rougher than 5x5; and
the dual demosaic under the hybrid is straighter than AMaZE alone. The
paragraph above had the sign of the strength's effect backwards, from
crops in which a sharper, noisier edge read as a straighter one.

**Edge-aware strength, tried and reverted.** Before the measurement
the remedy proposed above was built: the means' strength beyond 1
withheld wherever the finest band's local variance (the pyramid grid
the wavelets read, at scale 0) showed signal over the noise floor, on
a ramp over the structure ratio. On the bench it lost at every ramp
tried, more the earlier the ramp began (0.2 dB on Kodak at a sigma of
0.05 with the strength back to 1 at a ratio of 0.5, 0.05 dB at 2), and
on the ISO 32000 frame it changed almost nothing, since at that noise
few tiles clear the floor. Consistent with the table: the strength on
edges is earning its keep. Not kept. The edge waviness item is closed;
what is left at ISO 32000 is the coarse luminance mottling.

### 13u. The hybrid's second round trip, and a coarse error for the bench (2026-09-06)

The coarse luminance mottling of §13r needed a number before it could
be worked on. The bench gained one: **coarse**, the root mean square of
the lightness error (L*) averaged over 16x16 blocks of the interior.
Grain averages out of it and a blotch does not, and PSNR barely sees
the difference (a test in `metrics.rs` has a grain and a blotch of the
same amplitude: the blotch scores the better PSNR and thirty times the
coarse error).

The first run of it said the mottling was not the wavelets' coarse
scales at all. Linear domain, AMaZE, CPSNR / mean color error /
coarse, Kodak / McMaster, at a sigma of 0.05:

| denoiser                       | CPSNR         | ΔE          | coarse      |
|--------------------------------|---------------|-------------|-------------|
| none                           | 23.35 / 23.43 | 9.88 / 9.88 | 0.84 / 0.72 |
| wavelets alone                 | 28.56 / 26.54 | 4.38 / 6.88 | 0.93 / 1.68 |
| means alone                    | 28.95 / 27.34 | 4.52 / 6.35 | 0.83 / 1.42 |
| hybrid as shipped              | 28.23 / 26.00 | 4.77 / 7.25 | 1.58 / 2.47 |
| hybrid, wavelets from scale 5  | 28.31 / 26.17 | 4.83 / 7.06 | 1.52 / 2.38 |
| hybrid, wavelets from scale 7  | 28.31 / 26.17 | 4.83 / 7.06 | 1.52 / 2.38 |

The means alone leave the coarse error where the noise put it; the
hybrid doubled it, and lost 0.7 dB and a third of a ΔE against the
means alone, in the linear domain the engine actually works in (§13r
chose the hybrid from gamma-domain tables and by eye). Withholding the
wavelets from more scales did not help, and withholding them from
every scale (from 7: nothing shrunk, the image only transformed and
transformed back after the means) cost exactly as much. The damage was
the transform's. The means transformed in, worked, and transformed out
through the unbiased inverse (Mäkitalo and Foi), which is exact for a
value that is the mean of transformed noisy samples; the wavelets then
transformed the same values in again and out again through the same
inverse, and its correction, right the first time, was applied to a
value that no longer needed it. A shift in every level of the order of
the Poisson gain over four, invisible at base ISO, two percent of mid
grey at the bench's noisiest setting, more in the shadows. The
"mottling" was that shift, with the block-mean noise the means cannot
reach on top.

**One round trip.** `denoise_nlm` is now a transform, a core on the
stabilized samples (`denoise_stabilized`), and an inverse; the hybrid
transforms once, runs the core, rotates to luma and chroma, runs the
wavelets, and inverts once. Same table, the hybrid corrected:

| noise | CPSNR         | ΔE          | coarse      | means alone, for comparison |
|------:|---------------|-------------|-------------|-----------------------------|
| 0.05  | 29.00 / 27.35 | 4.33 / 6.28 | 0.84 / 1.44 | 28.95 / 27.34 · 4.52 / 6.35 |
| 0.03  | 31.01 / 29.72 | 3.57 / 4.87 | 0.50 / 0.74 | 30.98 / 29.71 · 3.64 / 4.90 |
| 0.01  | 33.79 / 32.13 | 2.76 / 3.79 | 0.28 / 0.32 | 33.79 / 32.13 · 2.76 / 3.79 |

Best or level on every measure at every noise level; the wavelets
alone keep 0.3 dB on Kodak at 0.01 and lose 0.5 on McMaster there. On
the R7 frame the window glass that read 129.0 in the old hybrid and
128.1 through the means alone now reads 128.2, and the color blotches
the means leave are still taken, with a little more of the texture
the old hybrid had smoothed into plastic.

**What the means leave, measured.** The other thing the wavelets had
wrong in the hybrid was the noise they assumed: white, unit variance,
in every band, when the means had just taken most of it. Pure unit
noise through the means as configured, then the band variances of
what comes out against the white values, at strength 2 with 5x5
patches, scales 0 to 5: 0.006, 0.009, 0.047, 0.35, 0.78, 1.08; at
strength 1: 0.03, 0.03, 0.07, 0.42, 0.85, 1.15. So the hybrid's
wavelet pass at scales 2 and 3, the ones §13r started it from, was
shrinking against twenty and three times the noise there was. That is
now measured at run time (`nlm::residual_band_ratios`: a 512-square
field of unit noise through the same core, a tenth of a second) and
the band variances scaled by it, so scale 2 is left nearly alone,
scale 3 shrunk against a third of the white noise, and 5 and up
against all of it. Flat noise is where the means average most, so the
measurement is the least they leave; in texture they leave more, and
the wavelets there err toward keeping it. `hybrid_from` stays at 2 and
now matters little.

Cost: a 24 MP develop with the hybrid is the sum of the two denoisers
(5.4 s today against 3.4 and 3.2 alone; the machine reads about a
fifth slower than §13r's numbers on every path, the calibration is not
where the time goes).

### 13v. The means' defaults re-swept in the linear domain (2026-09-06)

Every setting in §13r was chosen from gamma-domain tables against a
hybrid that was shifting levels (§13u). Re-swept with that fixed,
linear domain, AMaZE, the hybrid, CPSNR / mean ΔE / coarse, Kodak /
McMaster.

Means strength (`--denoise-strength` sets the means' in the hybrid):

| strength | noise 0.05                        | 0.03                              |
|---------:|-----------------------------------|-----------------------------------|
| 1        | 28.68 / 27.19 · 4.65 / 6.29 · 0.73 / 1.31 | 30.92 / 29.47 · 3.70 / 4.92 · 0.45 / 0.69 |
| 1.5      | 29.14 / 27.48 · 4.36 / 6.20 · 0.79 / 1.37 | 31.20 / 29.80 · 3.53 / 4.84 · 0.48 / 0.73 |
| 2        | 29.00 / 27.35 · 4.33 / 6.28 · 0.84 / 1.44 | 31.11 / 29.81 · 3.51 / 4.88 · 0.51 / 0.76 |
| 3        | 28.34 / 26.83 · 4.48 / 6.58 · 0.93 / 1.55 | 30.59 / 29.49 · 3.60 / 5.06 · 0.58 / 0.84 |

At 0.01, 1.5 against the ramp's 1: 33.79 / 32.19 against 33.79 /
32.13. So 1.5 at every noise, and the ramp is gone (`AUTO_STRENGTH`).
The dissimilarity scale [`NORM`] was swept too (0.6, 0.9, 1.35) and
is the same knob: its rows are the strength rows at 3, 2 and 1.33.

Patch size at 0.05: 3x3 gives 29.21 / 27.53 · 4.24 / 6.14 against
5x5's 29.00 / 27.35 · 4.33 / 6.28; at 0.03 5x5 loses 0.2 dB; at
0.01 5x5 loses 0.16. So 3x3 at every noise, and the switch to 5x5
above a sigma of 0.04 is gone. The chroma multiplier (1, 2, 4) and
`hybrid_from` (1, 2, 3) are flat to a hundredth of a dB and stay.

Both together, against the §13u defaults: 0.05: 29.21 / 27.57 · 4.29 /
6.08 · 0.78 / 1.37 (from 29.00 / 27.35 · 4.33 / 6.28); 0.03: 31.31 /
29.91 · 3.46 / 4.75 (from 31.01 / 29.72 · 3.57 / 4.87); 0.01: 33.82 /
32.23 · 2.71 / 3.77 (from 33.79 / 32.13 · 2.76 / 3.79). The gamma-domain
score at 0.05 is 29.15 / 29.01, level with §13r's best.

**The real frame disagrees a little.** On the R7 at ISO 32000 the new
settings leave a fine grain the old ones did not, small but there: the
thin lines' row-to-row step (§13t) goes from 0.31 to 0.45 px on the
vertical one and 0.24 to 0.32 on the horizontal, the glass crop's grey
deviation from 8.03 to 8.45, and it is the patch that does most of it
(3x3 at strength 2: 0.43; 5x5 at 1.5: 0.34). Two readings: the bench's
references carry grain of their own (§13n), which at a sigma of 0.05
is a fifth of the noise and rewards a denoiser that leaves a little;
or the frame's noise is not the bench's (its channels differ two to
one, its highlights are clipped, its demosaic is three quarters VNG4).
The rule here is that the bench decides and the crops veto what PSNR
cannot see, and a slight grain is something PSNR sees; so the bench's
settings ship. If a print at ISO 32000 wants the smoother look, the
strength slider is where it lives.
